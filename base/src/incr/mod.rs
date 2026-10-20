//! The incremental evaluator behind `Model::set_incremental`. Off, the model
//! is the shipped engine, which stays the reference the incremental results
//! are checked against.

pub(crate) mod ctx;
pub(crate) mod nodes;
mod read;

#[cfg(test)]
mod miri_tests;

use std::cell::Cell;
use std::rc::Rc;

use incr_compute::{Incr, NodeTable, Runtime};

use crate::model::Model;
use crate::types::FormulaValue;
use nodes::{Computed, Content, Dimension, Position};

/// The frames a compute may nest before its reads replay. Each frame holds
/// a formula's evaluation, a few KB of stack in release and about 16 KB in
/// debug, so this keeps a deep chain inside a 2 MiB thread.
pub(crate) const MAX_FRAMES: u32 = if cfg!(debug_assertions) { 16 } else { 64 };

/// The frame budget beside the engine's depth limit `max_depth`. A limit of
/// one makes the reference unwind at every formula, and the incremental
/// evaluator then replays at every read, so that both paths get exercised.
pub(crate) fn frame_budget(max_depth: usize) -> u32 {
    if max_depth == 1 {
        1
    } else {
        MAX_FRAMES
    }
}

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
    /// Bumped on every recalc; read by the volatile functions.
    pub(crate) epoch: Incr<u64>,
    /// The frame and byte budgets last given to `rt`.
    budgets: Cell<(u32, usize)>,
}

impl IncrState {
    fn new(max_depth: usize, max_stack: usize) -> IncrState {
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
        let state = IncrState {
            content: rt.create_table(),
            value: rt.create_table(),
            tile: rt.create_table(),
            dimension: rt.create_table(),
            epoch: rt.create_input(0),
            budgets: Cell::new((0, 0)),
            rt,
        };
        state.follow_limits(max_depth, max_stack);
        state
    }

    /// Sets the budgets from the engine's limits. The byte budget is the
    /// engine's own stack limit: both measure from the outermost formula, so
    /// a compute stops nesting where the reference would unwind.
    pub(crate) fn follow_limits(&self, max_depth: usize, max_stack: usize) {
        let frames = frame_budget(max_depth);
        self.rt.set_max_frames(frames);
        self.rt.set_max_stack_bytes(max_stack);
        self.budgets.set((frames, max_stack));
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn budgets(&self) -> (u32, usize) {
        self.budgets.get()
    }
}

impl Model<'_> {
    /// Switches the incremental evaluator on or off. Switching it on when it
    /// is on keeps the state it has.
    pub fn set_incremental(&mut self, on: bool) {
        if on != self.is_incremental() {
            let limits = (self.evaluation.max_depth, self.evaluation.max_stack);
            self.incr = on.then(|| Rc::new(IncrState::new(limits.0, limits.1)));
        }
    }

    /// Whether `set_incremental(true)` is in effect.
    pub fn is_incremental(&self) -> bool {
        self.incr.is_some()
    }
}
