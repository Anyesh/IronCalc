//! What was changed in the cells of a sheet, for whoever keeps something
//! computed from them and needs to know what to compute again.

/// A change to the cells of a sheet, as `SheetData::changes` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The cell at a position was written, replaced or removed.
    Cell { row: i32, column: i32 },
    /// The row was replaced or removed whole.
    Row(i32),
}
