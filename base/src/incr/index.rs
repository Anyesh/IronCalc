//! Indices the hot paths read in place of hashed lookups. They hold handles
//! of `IncrState`'s nodes; a handle kept here stays valid because the state
//! never deletes a node.

use incr_compute::{ErasedIncr, Incr};

use super::nodes::{Computed, Dimension, Position};

/// The watched `value` nodes and their positions, found by node slot.
#[derive(Default)]
pub(crate) struct Watched {
    /// Per node slot, one plus the index of its entry, or 0.
    by_slot: Vec<u32>,
    entries: Vec<(Incr<Computed>, Position)>,
}

impl Watched {
    pub(crate) fn insert(&mut self, handle: Incr<Computed>, p: Position) {
        let slot = handle.slot() as usize;
        if self.by_slot.len() <= slot {
            self.by_slot.resize(slot + 1, 0);
        }
        match self.by_slot[slot] {
            0 => {
                self.entries.push((handle, p));
                self.by_slot[slot] = self.entries.len() as u32;
            }
            index => self.entries[index as usize - 1] = (handle, p),
        }
    }

    #[inline]
    pub(crate) fn get(&self, handle: ErasedIncr) -> Option<(Incr<Computed>, Position)> {
        let index = *self.by_slot.get(handle.slot() as usize)?;
        let &(watched, p) = self.entries.get((index as usize).checked_sub(1)?)?;
        (watched.erase() == handle).then_some((watched, p))
    }

    pub(crate) fn contains(&self, handle: ErasedIncr) -> bool {
        self.get(handle).is_some()
    }

    /// True if `handle` was watched.
    pub(crate) fn remove(&mut self, handle: ErasedIncr) -> bool {
        if !self.contains(handle) {
            return false;
        }
        let slot = handle.slot() as usize;
        let index = self.by_slot[slot] as usize - 1;
        self.by_slot[slot] = 0;
        self.entries.swap_remove(index);
        if let Some(&(moved, _)) = self.entries.get(index) {
            self.by_slot[moved.slot() as usize] = index as u32 + 1;
        }
        true
    }

    #[cfg(debug_assertions)]
    pub(crate) fn positions(&self) -> Vec<Position> {
        self.entries.iter().map(|&(_, p)| p).collect()
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
