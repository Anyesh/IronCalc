//! Indices the hot paths read in place of hashed lookups. They hold handles
//! of `IncrState`'s tables, which stay the creators; a handle kept here
//! stays valid because the state never removes a node from its tables.

use std::cell::RefCell;
use std::collections::HashMap;

use incr_compute::{FxBuildHasher, Incr, Runtime};

use super::nodes::{Dimension, Position};

const PAGE_ROWS: i32 = 32;

type Page<T> = [Option<Incr<T>>; PAGE_ROWS as usize];

/// `(sheet, column, row / PAGE_ROWS)`.
type PageKey = (u32, i32, i32);

/// A table's live nodes by position, in pages of rows of one column, so that
/// a walk down a column reads adjacent memory and a sparse sheet costs at
/// most a page per node.
pub(crate) struct CellIndex<T: 'static> {
    pages: RefCell<HashMap<PageKey, Box<Page<T>>, FxBuildHasher>>,
}

impl<T: 'static> Default for CellIndex<T> {
    fn default() -> Self {
        CellIndex {
            pages: RefCell::new(HashMap::with_hasher(FxBuildHasher)),
        }
    }
}

impl<T: 'static> CellIndex<T> {
    /// The node at `p`, or the one `insert` finds or creates in the table.
    #[inline]
    pub(crate) fn get_or_insert(
        &self,
        rt: &Runtime,
        p: Position,
        insert: impl FnOnce() -> Incr<T>,
    ) -> Incr<T> {
        let (sheet, row, column) = p;
        let page = (sheet, column, row.div_euclid(PAGE_ROWS));
        let at = row.rem_euclid(PAGE_ROWS) as usize;
        if let Some(handle) = self.pages.borrow().get(&page).and_then(|page| page[at]) {
            return handle;
        }
        let handle = insert();
        // A doomed frame's miss returns a pending handle that names no node.
        if rt.is_alive(handle) {
            self.pages
                .borrow_mut()
                .entry(page)
                .or_insert_with(|| Box::new([None; PAGE_ROWS as usize]))[at] = Some(handle);
        }
        handle
    }
}

/// Per sheet, the used area last read or set, and its `dimension` node once
/// one is alive.
#[derive(Default)]
pub(crate) struct Dimensions(Vec<Option<(Dimension, Option<Incr<Dimension>>)>>);

impl Dimensions {
    #[inline]
    pub(crate) fn get(&self, sheet: u32) -> Option<(Dimension, Option<Incr<Dimension>>)> {
        self.0.get(sheet as usize).copied().flatten()
    }

    pub(crate) fn set(&mut self, sheet: u32, d: Dimension, node: Option<Incr<Dimension>>) {
        let sheet = sheet as usize;
        if self.0.len() <= sheet {
            self.0.resize(sheet + 1, None);
        }
        self.0[sheet] = Some((d, node));
    }

    pub(crate) fn known(&self) -> Vec<(u32, Dimension)> {
        self.0
            .iter()
            .enumerate()
            .filter_map(|(sheet, known)| known.map(|(d, _)| (sheet as u32, d)))
            .collect()
    }
}
