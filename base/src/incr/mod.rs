//! The incremental evaluator behind `Model::set_incremental`. Off, the model
//! is the shipped engine, which stays the reference the incremental results
//! are checked against.

// Read by the read path, which waits for the plan/apply split of the writeback.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod ctx;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod nodes;

use std::rc::Rc;

use crate::model::Model;

/// Lives in an `Rc` so that it stays put while the `Model` that owns it is
/// moved, and so that a compute can reach the runtime without borrowing the
/// model.
pub(crate) struct IncrState {}

impl Model<'_> {
    /// Switches the incremental evaluator on or off. Switching it on when it
    /// is on keeps the state it has.
    pub fn set_incremental(&mut self, on: bool) {
        if on != self.is_incremental() {
            self.incr = on.then(|| Rc::new(IncrState {}));
        }
    }

    /// Whether `set_incremental(true)` is in effect.
    pub fn is_incremental(&self) -> bool {
        self.incr.is_some()
    }
}
