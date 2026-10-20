//! The random numbers of a recalc. A formula can run more than once in one
//! recalc (a restarted pass, a replayed or repeated compute, or the reference
//! and the incremental evaluator both running it), and each run must draw
//! what the first one drew, so every draw is kept, by the cell that made it
//! and how many draws that cell's run had made before it.

use std::collections::HashMap;

use crate::expressions::types::CellReferenceIndex;
use crate::model::Model;

type CellKey = (u32, i32, i32);

#[derive(Default)]
pub(crate) struct VolatileLog {
    draws: HashMap<(CellKey, u32), f64>,
    /// How many draws the current run of each formula has made.
    runs: HashMap<CellKey, u32>,
}

impl VolatileLog {
    /// Starts a new recalc: nothing drawn yet.
    pub(crate) fn clear(&mut self) {
        self.draws.clear();
        self.runs.clear();
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn random() -> f64 {
    rand::random()
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn random() -> f64 {
    use js_sys::Math;
    Math::random()
}

fn key(cell: CellReferenceIndex) -> CellKey {
    (cell.sheet, cell.row, cell.column)
}

impl Model<'_> {
    /// A run of the formula at `cell` starts: its next draw is its first.
    #[inline]
    pub(crate) fn start_formula_run(&mut self, cell: CellReferenceIndex) {
        if !self.volatile.runs.is_empty() {
            self.volatile.runs.remove(&key(cell));
        }
    }

    /// A random number in [0, 1) for the formula at `cell`.
    pub(crate) fn draw_random(&mut self, cell: CellReferenceIndex) -> f64 {
        self.note_epoch_read();
        let key = key(cell);
        let count = self.volatile.runs.entry(key).or_insert(0);
        let index = *count;
        *count += 1;
        *self
            .volatile
            .draws
            .entry((key, index))
            .or_insert_with(random)
    }
}
