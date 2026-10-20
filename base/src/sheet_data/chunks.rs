//! The rows of a sheet, by position: chunks of consecutive rows, allocated as
//! they are needed.

use std::borrow::Cow;

use bitcode::{Decode, Encode};

use super::changes::Change;
use super::row::Row;
use super::value::CellValueMut;
use crate::constants::LAST_ROW;
use crate::types::Cell;

const CHUNK_BITS: u32 = 10;
const CHUNK_ROWS: usize = 1 << CHUNK_BITS;
const CHUNK_MASK: usize = CHUNK_ROWS - 1;

/// The cells of a worksheet, by row and column. Both are 1-indexed.
///
/// A row can exist without cells: `set_row` with no cells creates one. The xlsx
/// importer does that for rows that carry a height or a style and nothing
/// else, and the exporter writes a `<row>` element for every row that exists
/// here, so such rows survive a round trip. Removing the last cell of a row
/// with `remove_cell` removes the row.
///
/// Rows outside the sheet, below 1 or beyond the last row, cannot hold cells:
/// writing to them does nothing and reading them finds nothing.
///
/// Asked to (`log_changes`), the sheet logs every change made through
/// `cell_mut`, `set_cell`, `remove_cell`, `remove_row` and `set_row`, and
/// none made through `cell_mut_value`. The log is neither saved nor compared.
#[derive(Encode, Decode, Debug, Clone, Default)]
pub struct SheetData {
    /// `chunks[i]` holds rows `i * CHUNK_ROWS ..`, if any of them exists.
    chunks: Vec<Option<Box<Chunk>>>,
    /// How many rows exist.
    row_count: usize,
    /// The changes since the log was last taken, if they are being logged.
    #[bitcode(skip)]
    changes: Option<Vec<Change>>,
}

#[derive(Encode, Decode, Debug, Clone)]
struct Chunk {
    /// Always `CHUNK_ROWS` long.
    rows: Vec<Option<Row>>,
    /// How many of them exist. A chunk with none is freed.
    used: usize,
}

impl Chunk {
    fn new() -> Self {
        let mut rows = Vec::new();
        rows.resize_with(CHUNK_ROWS, || None);
        Chunk { rows, used: 0 }
    }
}

/// The chunk and the place in it of a row, if the row is within the sheet.
#[inline]
fn locate(row: i32) -> Option<(usize, usize)> {
    if !(1..=LAST_ROW).contains(&row) {
        return None;
    }
    let row = row as usize;
    Some((row >> CHUNK_BITS, row & CHUNK_MASK))
}

impl SheetData {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub(super) fn row(&self, row: i32) -> Option<&Row> {
        let (chunk, index) = locate(row)?;
        self.chunks.get(chunk)?.as_ref()?.rows[index].as_ref()
    }

    #[inline]
    fn row_mut(&mut self, row: i32) -> Option<&mut Row> {
        let (chunk, index) = locate(row)?;
        self.chunks.get_mut(chunk)?.as_mut()?.rows[index].as_mut()
    }

    /// The row, created if it does not exist. `None` outside the sheet.
    fn row_or_insert(&mut self, row: i32) -> Option<&mut Row> {
        let (chunk, index) = locate(row)?;
        if chunk >= self.chunks.len() {
            self.chunks.resize_with(chunk + 1, || None);
        }
        let chunk = self.chunks[chunk].get_or_insert_with(|| Box::new(Chunk::new()));
        if chunk.rows[index].is_none() {
            chunk.rows[index] = Some(Row::default());
            chunk.used += 1;
            self.row_count += 1;
        }
        chunk.rows[index].as_mut()
    }

    /// Every row that exists with its number, in ascending order.
    fn rows_in_order(&self) -> impl Iterator<Item = (i32, &Row)> {
        self.chunks
            .iter()
            .enumerate()
            .filter_map(|(chunk_index, chunk)| Some((chunk_index, chunk.as_ref()?)))
            .flat_map(|(chunk_index, chunk)| {
                chunk
                    .rows
                    .iter()
                    .enumerate()
                    .filter_map(move |(index, row)| {
                        let number = (chunk_index << CHUNK_BITS) | index;
                        Some((number as i32, row.as_ref()?))
                    })
            })
    }

    /// The cell at a position, if there is one.
    #[inline]
    pub fn cell(&self, row: i32, column: i32) -> Option<Cow<'_, Cell>> {
        self.row(row)?.get(column)
    }

    /// Starts logging changes (see `changes`), or stops and forgets them.
    pub fn log_changes(&mut self, on: bool) {
        self.changes = on.then(Vec::new);
    }

    /// The changes since the log was started or last taken, oldest first. A
    /// position can be there more than once, and for a change that wrote what
    /// was already there.
    pub fn changes(&self) -> &[Change] {
        self.changes.as_deref().unwrap_or_default()
    }

    /// The changes, leaving the log empty and still logging.
    pub fn take_changes(&mut self) -> Vec<Change> {
        self.changes
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    #[inline]
    fn log(&mut self, change: Change) {
        if let Some(changes) = &mut self.changes {
            changes.push(change);
        }
    }

    /// The cell at a position, to be changed in place.
    pub fn cell_mut(&mut self, row: i32, column: i32) -> Option<&mut Cell> {
        if self.changes.is_some() && self.cell(row, column).is_some() {
            self.log(Change::Cell { row, column });
        }
        self.row_mut(row)?.get_mut(column)
    }

    /// The value of the formula, array formula or spill cell at a position, to
    /// be changed in place. This is the one change that is not logged: it is
    /// how computed values are written back.
    pub fn cell_mut_value(&mut self, row: i32, column: i32) -> Option<CellValueMut<'_>> {
        CellValueMut::of(self.row_mut(row)?.get_held_mut(column)?)
    }

    /// Puts a cell at a position and returns the one that was there.
    pub fn set_cell(&mut self, row: i32, column: i32, cell: Cell) -> Option<Cell> {
        let old = self.row_or_insert(row)?.insert(column, cell);
        self.log(Change::Cell { row, column });
        old
    }

    /// Removes the cell at a position and returns it.
    pub fn remove_cell(&mut self, row: i32, column: i32) -> Option<Cell> {
        let row_data = self.row_mut(row)?;
        let cell = row_data.remove(column)?;
        if row_data.is_empty() {
            self.drop_row(row);
        }
        self.log(Change::Cell { row, column });
        Some(cell)
    }

    /// True if the sheet has no rows.
    pub fn is_empty(&self) -> bool {
        self.row_count == 0
    }

    /// Every cell of the sheet with its row and column, in natural order:
    /// by row, and within a row by column.
    pub fn cells(&self) -> impl Iterator<Item = (i32, i32, Cow<'_, Cell>)> {
        self.rows_in_order().flat_map(|(row, row_data)| {
            row_data
                .iter()
                .map(move |(column, cell)| (row, column, cell))
        })
    }

    /// The rows that exist, in ascending order.
    pub fn rows(&self) -> Vec<i32> {
        self.rows_in_order().map(|(row, _)| row).collect()
    }

    /// The columns of a row that have cells, in ascending order.
    pub fn columns_in_row(&self, row: i32) -> Vec<i32> {
        match self.row(row) {
            Some(row_data) => row_data.iter().map(|(column, _)| column).collect(),
            None => Vec::new(),
        }
    }

    /// The cells of a row with their column, in ascending order of column.
    pub fn cells_in_row(&self, row: i32) -> Vec<(i32, Cow<'_, Cell>)> {
        match self.row(row) {
            Some(row_data) => row_data.iter().collect(),
            None => Vec::new(),
        }
    }

    /// The cells of a row from `first_column` to `last_column`, both included,
    /// with their column, in ascending order of column. Reading a stretch of a
    /// row this way finds the row once, not once per cell.
    pub fn cells_in_row_between(
        &self,
        row: i32,
        first_column: i32,
        last_column: i32,
    ) -> impl Iterator<Item = (i32, Cow<'_, Cell>)> {
        self.row(row)
            .map(|row_data| row_data.iter_from(first_column))
            .into_iter()
            .flatten()
            .take_while(move |(column, _)| *column <= last_column)
    }

    /// The numbers of a row from `first_column` to `last_column`, both
    /// included, if the row holds them as numbers and nothing else, with none
    /// missing. `None` is no news about the row: its cells say what it holds.
    #[inline]
    pub fn numbers_in_row_between(
        &self,
        row: i32,
        first_column: i32,
        last_column: i32,
    ) -> Option<&[f64]> {
        match self.row(row)? {
            Row::Numbers {
                first_column: row_first_column,
                values,
                ..
            } => {
                let first = Row::offset(*row_first_column, values, first_column)?;
                let last = Row::offset(*row_first_column, values, last_column)?;
                values.get(first..=last)
            }
            Row::Cells(_) => None,
        }
    }

    /// Removes every cell of a row, and the row.
    pub fn remove_row(&mut self, row: i32) {
        if self.drop_row(row) {
            self.log(Change::Row(row));
        }
    }

    /// Removes the row, unlogged. True if it existed.
    fn drop_row(&mut self, row: i32) -> bool {
        let Some((chunk_index, index)) = locate(row) else {
            return false;
        };
        let Some(Some(chunk)) = self.chunks.get_mut(chunk_index) else {
            return false;
        };
        if chunk.rows[index].take().is_none() {
            return false;
        }
        chunk.used -= 1;
        self.row_count -= 1;
        if chunk.used == 0 {
            self.chunks[chunk_index] = None;
        }
        true
    }

    /// Replaces the cells of a row. The row exists afterwards, even with no
    /// cells. If a column is given twice the last one stays.
    pub fn set_row(&mut self, row: i32, cells: impl IntoIterator<Item = (i32, Cell)>) {
        let Some(row_data) = self.row_or_insert(row) else {
            return;
        };
        // Collected at its exact size: a row grown cell by cell from empty
        // starts with room for four, and a sheet with a million short rows
        // paid for that four times over.
        let mut cells: Vec<(i32, Cell)> = cells.into_iter().collect();
        // A file gives the cells of a row from left to right, and then there
        // is nothing to sort and no column given twice.
        if !cells.is_sorted_by(|earlier, later| earlier.0 < later.0) {
            cells.sort_by_key(|(column, _)| *column);
            // Of two cells in the same column the later one stays: it is moved
            // into the slot of the earlier one, which `dedup_by` then keeps.
            cells.dedup_by(|later, earlier| {
                if later.0 == earlier.0 {
                    std::mem::swap(&mut later.1, &mut earlier.1);
                    true
                } else {
                    false
                }
            });
        }
        cells.shrink_to_fit();
        *row_data = Row::from_cells(cells);
        self.log(Change::Row(row));
    }
}

/// Two sheets are equal when they have the same rows and the same cells.
/// Which chunks happen to be allocated is not part of that.
impl PartialEq for SheetData {
    fn eq(&self, other: &Self) -> bool {
        self.row_count == other.row_count && self.rows_in_order().eq(other.rows_in_order())
    }
}
