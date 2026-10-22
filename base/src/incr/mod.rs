//! The incremental evaluator behind `Model::set_incremental`. Off, the model
//! is the shipped engine, which stays the reference the incremental results
//! are checked against.
//!
//! It learns of edits through `Model`'s methods. A write through the public
//! `Model::workbook` (`Worksheet::set_row_hidden`, `workbook.tables`, ...)
//! bypasses it, as it bypasses the shipped engine's parsed-structure caches,
//! and leaves stale values.

pub(crate) mod ctx;
mod index;
pub(crate) mod nodes;
mod read;
mod recalc;

#[cfg(test)]
mod miri_tests;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use incr_compute::{
    Cycle, ErasedIncr, FxBuildHasher, GridKey, GridTable, Incr, NodeTable, Runtime,
};

use crate::model::Model;
use crate::types::FormulaValue;
use crate::UserModel;
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
    pub(crate) content: GridTable<Content>,
    pub(crate) value: GridTable<Computed>,
    /// A version per 64×64 block, bumped when a cell of the block changes.
    pub(crate) tile: NodeTable<Position, u64>,
    pub(crate) dimension: NodeTable<u32, Dimension>,
    /// Bumped on every recalc; read by the volatile functions.
    pub(crate) epoch: Incr<u64>,
    /// The frame and byte budgets last given to `rt`.
    budgets: Cell<(u32, usize)>,
    /// The formula positions with a watched `value` node, by that node.
    pub(crate) watched: RefCell<index::Watched>,
    /// Members of cycles the incremental evaluator found, pinned to what the
    /// reference stored: their readers take that from `content`.
    pub(crate) cycle_pins: RefCell<HashSet<Position, FxBuildHasher>>,
    /// The dynamic anchors and their spill cells as the last rebuild or
    /// reference recalc left them: their values are the reference's.
    pub(crate) dynamic: RefCell<HashSet<Position, FxBuildHasher>>,
    /// Formulas whose last compute read a cycle pin, or a tainted formula.
    pub(crate) tainted: RefCell<HashSet<Position, FxBuildHasher>>,
    /// The value of each `dimension` node, readable without depending on it.
    pub(crate) dimensions: RefCell<index::Dimensions>,
    /// The position of every `value` node not in `watched`, by slot: the
    /// cycle fallback is handed only the node.
    pub(crate) value_positions: RefCell<HashMap<u32, Position, FxBuildHasher>>,
    /// One per running compute, innermost last.
    pub(crate) frames: RefCell<Vec<read::Frame>>,
    /// Formulas to compute and write back whether or not a change reached
    /// their node: a cell a change rewrote holds no value of the node's.
    pub(crate) pending: RefCell<Vec<Position>>,
    /// Reached formulas the next drain computes first: the pinned, tainted
    /// and rewritten ones a reference recalc still computes, or the batch a
    /// drain that needs no reference starts from.
    pub(crate) carry: RefCell<Vec<ErasedIncr>>,
    /// Reached formulas a reference recalc left to the reference and did not
    /// compute; see `recalc::Deferred`.
    pub(crate) deferred: RefCell<recalc::Deferred>,
    /// Set when a formula becomes tainted that was not.
    pub(crate) newly_tainted: Cell<bool>,
    /// Whether the last reference recalc's restarts marked an anchor
    /// circular: the marks last one evaluation, so the shipped engine would
    /// restart on that anchor again.
    pub(crate) marks_recur: Cell<bool>,
    /// What the state was built from; see `recalc::Fingerprint`.
    pub(crate) fingerprint: RefCell<Option<recalc::Fingerprint>>,
    /// Set by `Model::metadata_edited`: the next recalc rebuilds.
    pub(crate) metadata_edited: Cell<bool>,
    pub(crate) reference_runs: Cell<u32>,
    pub(crate) rebuilds: Cell<u32>,
    pub(crate) tile_reads: Cell<u64>,
    /// Runs of a `value` node's compute.
    pub(crate) computes: Cell<u64>,
    /// Recalcs that walked every cell of a sheet to find its used area.
    pub(crate) used_area_scans: Cell<u64>,
    /// Walks of every cell for the dynamic anchors and their spill cells.
    pub(crate) anchor_scans: Cell<u64>,
}

/// A cell's key in the `content` and `value` tables. Rows and columns are
/// positive.
pub(crate) fn grid((sheet, row, column): Position) -> GridKey {
    GridKey::new(sheet, row as u32, column as u32)
}

impl IncrState {
    fn new(max_depth: usize, max_stack: usize) -> IncrState {
        let rt = Runtime::new();
        // Every type a replayable compute reads, so that a doomed read has
        // something to return.
        rt.set_placeholder(Content::Empty);
        rt.set_placeholder(Computed::new(FormulaValue::Unevaluated, Vec::new()));
        rt.set_placeholder(0_u64);
        rt.set_placeholder::<Dimension>((1, 1, 1, 1));
        rt.set_cycle_fallback(|_: &Cycle, node: Incr<Computed>| {
            ctx::with_model(|model| model.incr_circular_node(node))
        });
        let state = IncrState {
            content: rt.create_grid_table(),
            value: rt.create_grid_table(),
            tile: rt.create_table(),
            dimension: rt.create_table(),
            epoch: rt.create_input(0),
            budgets: Cell::new((0, 0)),
            watched: RefCell::default(),
            cycle_pins: RefCell::default(),
            dynamic: RefCell::default(),
            tainted: RefCell::default(),
            dimensions: RefCell::default(),
            value_positions: RefCell::default(),
            frames: RefCell::default(),
            pending: RefCell::default(),
            carry: RefCell::default(),
            deferred: RefCell::default(),
            newly_tainted: Cell::new(false),
            fingerprint: RefCell::new(None),
            metadata_edited: Cell::new(false),
            marks_recur: Cell::new(false),
            reference_runs: Cell::new(0),
            rebuilds: Cell::new(0),
            tile_reads: Cell::new(0),
            computes: Cell::new(0),
            used_area_scans: Cell::new(0),
            anchor_scans: Cell::new(0),
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
    /// is on keeps the state it has. Edit the workbook through `Model`'s
    /// methods while it is on: a write through `self.workbook`, such as
    /// `Worksheet::set_row_hidden`, is not seen, so a `SUBTOTAL(109, ...)`
    /// over that row keeps its old value.
    pub fn set_incremental(&mut self, on: bool) {
        if on != self.is_incremental() {
            let limits = (self.evaluation.max_depth, self.evaluation.max_stack);
            self.replace_incr(on.then(|| Rc::new(IncrState::new(limits.0, limits.1))));
        }
    }

    /// Every write of `self.incr` but the model's construction. `incr_ref`
    /// lends the state across `rt.get`, so no compute may replace it.
    pub(crate) fn replace_incr(&mut self, state: Option<Rc<IncrState>>) {
        debug_assert!(
            !ctx::is_armed(),
            "the incremental state was replaced inside a compute"
        );
        self.incr = state;
    }

    /// Whether `set_incremental(true)` is in effect.
    pub fn is_incremental(&self) -> bool {
        self.incr.is_some()
    }
}

impl UserModel<'_> {
    /// `Model::set_incremental` on the model this one edits.
    pub fn set_incremental(&mut self, on: bool) {
        self.model.set_incremental(on);
    }
}
