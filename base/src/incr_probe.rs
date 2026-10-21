//! What the incr benches read from a model with the incremental evaluator
//! on: the state's counters and the fingerprint check of a no-op `evaluate`.

use crate::model::Model;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    /// `evaluate` calls that fell back to the shipped evaluator.
    pub reference_runs: u32,
    /// Times the state was rebuilt from the cells.
    pub rebuilds: u32,
}

/// The counters of the incremental state, or `None` while it is off.
pub fn counts(model: &Model) -> Option<Counts> {
    model.incr.as_ref().map(|state| Counts {
        reference_runs: state.reference_runs.get(),
        rebuilds: state.rebuilds.get(),
    })
}

/// Runs the fingerprint check a no-op `evaluate` makes and returns whether
/// the stored fingerprint still matched. Panics while the incremental
/// evaluator is off.
pub fn fingerprint_check(model: &Model) -> bool {
    model.incr_fingerprint_check()
}
