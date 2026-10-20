//! The incremental evaluator behind `Model::set_incremental`. Off, the model
//! is the shipped engine, which stays the reference the incremental results
//! are checked against.

pub(crate) mod ctx;
pub(crate) mod nodes;
mod read;

use std::rc::Rc;

use incr_compute::{NodeTable, Runtime};

use crate::model::Model;
use crate::types::FormulaValue;
use nodes::{Computed, Content, Dimension, Position};

/// The frames a compute may nest before its reads replay. Each frame holds
/// a formula's evaluation, a few KB of stack in release and about 16 KB in
/// debug, so this keeps a deep chain inside a 2 MiB thread.
const MAX_FRAMES: u32 = if cfg!(debug_assertions) { 16 } else { 64 };

/// Lives in an `Rc` so that it stays put while the `Model` that owns it is
/// moved, and so that a compute can reach the runtime without borrowing the
/// model.
pub(crate) struct IncrState {
    pub(crate) rt: Runtime,
    pub(crate) content: NodeTable<Position, Content>,
    pub(crate) value: NodeTable<Position, Computed>,
    /// A version per 64×64 block, bumped when a cell of the block changes.
    pub(crate) tile: NodeTable<Position, u64>,
    pub(crate) dimension: NodeTable<u32, Dimension>,
}

impl IncrState {
    fn new() -> IncrState {
        let rt = Runtime::new();
        // Every type a replayable compute reads, so that a doomed read has
        // something to return.
        rt.set_placeholder(Content::Empty);
        rt.set_placeholder(Computed {
            v: FormulaValue::Unevaluated,
            spills: None,
        });
        rt.set_placeholder(0_u64);
        rt.set_placeholder::<Dimension>((1, 1, 1, 1));
        rt.set_max_frames(MAX_FRAMES);
        IncrState {
            content: rt.create_table(),
            value: rt.create_table(),
            tile: rt.create_table(),
            dimension: rt.create_table(),
            rt,
        }
    }
}

impl Model<'_> {
    /// Switches the incremental evaluator on or off. Switching it on when it
    /// is on keeps the state it has.
    pub fn set_incremental(&mut self, on: bool) {
        if on != self.is_incremental() {
            self.incr = on.then(|| Rc::new(IncrState::new()));
        }
    }

    /// Whether `set_incremental(true)` is in effect.
    pub fn is_incremental(&self) -> bool {
        self.incr.is_some()
    }
}
