//! What `evaluate` does while the incremental evaluator is on. The change
//! logs say which cells changed; their `content`, tile and used-area inputs
//! are set, which dirties the formulas that read them, and the watched
//! `value` nodes that changes reached are computed and written back.
//!
//! The reference (the shipped evaluation) runs instead of trusting those
//! values when a recalc touches what only it computes exactly: a dynamic
//! array, a cycle, or a formula that reads one. It runs at most once per
//! recalc, and the members of the cycles found after it are pinned to the
//! values it stored.

use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use super::nodes::{tile_of, Computed, Content, Position};
use super::IncrState;
use crate::expressions::types::CellReferenceIndex;
use crate::model::Model;
use crate::sheet_data::Change;
use crate::types::{ArrayKind, Cell, FormulaValue};

/// What the state was built from besides the cells. When it no longer
/// matches, the state is rebuilt.
pub(crate) struct Fingerprint {
    metadata: u64,
    /// Per sheet, how many formulas it had and a hash of them: a sheet's
    /// formula list only grows while the indices into it stay valid.
    formulas: Vec<(usize, u64)>,
}

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn position(p: Position) -> CellReferenceIndex {
    CellReferenceIndex {
        sheet: p.0,
        row: p.1,
        column: p.2,
    }
}

impl Model<'_> {
    fn fingerprint(&self) -> Fingerprint {
        let workbook = &self.workbook;
        let mut hasher = DefaultHasher::new();
        workbook.name.hash(&mut hasher);
        (
            format!("{:?}", workbook.settings),
            self.locale as *const _ as usize,
        )
            .hash(&mut hasher);
        (self.language as *const _ as usize).hash(&mut hasher);
        for name in &workbook.defined_names {
            (&name.name, &name.formula, name.sheet_id).hash(&mut hasher);
        }
        let tables: BTreeSet<String> = workbook
            .tables
            .iter()
            .map(|(name, table)| format!("{name}{table:?}"))
            .collect();
        tables.hash(&mut hasher);
        for worksheet in &workbook.worksheets {
            (&worksheet.name, worksheet.sheet_id).hash(&mut hasher);
            format!("{:?}{:?}", worksheet.state, worksheet.merged_cells).hash(&mut hasher);
            for row in worksheet.rows.iter().filter(|row| row.hidden) {
                row.r.hash(&mut hasher);
            }
        }
        Fingerprint {
            metadata: hasher.finish(),
            formulas: workbook
                .worksheets
                .iter()
                .map(|w| (w.shared_formulas.len(), hash_of(&w.shared_formulas)))
                .collect(),
        }
    }

    fn fingerprint_matches(&self, built: &Fingerprint) -> bool {
        let now = self.fingerprint();
        now.metadata == built.metadata
            && now.formulas.len() == built.formulas.len()
            && self.workbook.worksheets.iter().zip(&built.formulas).all(
                |(worksheet, &(len, hash))| {
                    worksheet.shared_formulas.len() >= len
                        && hash_of(&worksheet.shared_formulas[..len]) == hash
                },
            )
    }

    fn a_row_changed(&self) -> bool {
        self.workbook.worksheets.iter().any(|worksheet| {
            worksheet
                .sheet_data
                .changes()
                .iter()
                .any(|change| matches!(change, Change::Row(_)))
        })
    }

    /// `evaluate` with the incremental evaluator on.
    pub(crate) fn incr_evaluate(&mut self) {
        let state = self.incr_state();
        let current = state
            .fingerprint
            .borrow()
            .as_ref()
            .is_some_and(|built| self.fingerprint_matches(built));
        let (state, mut reference) = if !current || self.a_row_changed() {
            // An anchor the edit removed is still in the order until the
            // reference drops it.
            let had_anchors = !state.dynamic.borrow().is_empty();
            let state = self.incr_rebuild(&state);
            let reference = had_anchors || !state.dynamic.borrow().is_empty();
            (state, reference)
        } else {
            let reference = self.incr_take_changes(&state);
            (state, reference)
        };
        state.rt.set(state.epoch, state.rt.get(state.epoch) + 1);
        reference |= state.marks_recur.get();
        reference |= self.incr_drain(&state, false);
        if reference {
            state.reference_runs.set(state.reference_runs.get() + 1);
            state.marks_recur.set(self.evaluate_reference());
            self.incr_resync(&state);
        } else {
            self.evaluation.restarts_in_last_evaluation = 0;
            self.evaluate_conditional_formatting();
        }
        *state.fingerprint.borrow_mut() = Some(self.fingerprint());
        #[cfg(debug_assertions)]
        self.incr_check(&state);
    }

    /// A fresh state with a watched `value` node for every formula, none of
    /// them computed yet.
    fn incr_rebuild(&mut self, old: &IncrState) -> Rc<IncrState> {
        let state = Rc::new(IncrState::new(
            self.evaluation.max_depth,
            self.evaluation.max_stack,
        ));
        state.reference_runs.set(old.reference_runs.get());
        state.rebuilds.set(old.rebuilds.get() + 1);
        state.tile_reads.set(old.tile_reads.get());
        self.incr = Some(Rc::clone(&state));
        let mut formulas = Vec::new();
        for (sheet, worksheet) in self.workbook.worksheets.iter_mut().enumerate() {
            worksheet.sheet_data.log_changes(true);
            worksheet.sheet_data.take_changes();
            for (row, column, cell) in worksheet.sheet_data.cells() {
                if matches!(*cell, Cell::CellFormula { .. } | Cell::ArrayFormula { .. }) {
                    formulas.push((sheet as u32, row, column));
                }
            }
        }
        for p in formulas {
            self.incr_watch(&state, p);
        }
        self.incr_find_dynamic(&state);
        state
    }

    fn incr_watch(&self, state: &IncrState, p: Position) {
        let handle = self.incr_value_node(position(p));
        state.rt.watch(handle);
        state.watched.borrow_mut().insert(handle.into(), p);
    }

    fn incr_find_dynamic(&self, state: &IncrState) {
        let dynamic = self
            .workbook
            .worksheets
            .iter()
            .enumerate()
            .flat_map(|(sheet, worksheet)| {
                worksheet
                    .sheet_data
                    .cells()
                    .filter(|(_, _, cell)| match &**cell {
                        Cell::ArrayFormula { kind, .. } => *kind == ArrayKind::Dynamic,
                        Cell::SpillCell { a, .. } => matches!(
                            worksheet.cell(a.0, a.1).as_deref(),
                            Some(Cell::ArrayFormula {
                                kind: ArrayKind::Dynamic,
                                ..
                            })
                        ),
                        _ => false,
                    })
                    .map(move |(row, column, _)| (sheet as u32, row, column))
                    .collect::<Vec<_>>()
            })
            .collect();
        *state.dynamic.borrow_mut() = dynamic;
    }

    /// Sets the inputs the logged changes reached, and watches the formulas
    /// they created. True if a change needs the reference: it touched a
    /// position whose value is the reference's, or a tainted formula, or
    /// made a dynamic anchor.
    fn incr_take_changes(&mut self, state: &IncrState) -> bool {
        let mut touched = BTreeSet::new();
        for (sheet, worksheet) in self.workbook.worksheets.iter_mut().enumerate() {
            for change in worksheet.sheet_data.take_changes() {
                if let Change::Cell { row, column } = change {
                    touched.insert((sheet as u32, row, column));
                }
            }
        }
        let mut reference = false;
        let mut sheets = BTreeSet::new();
        for p in touched {
            sheets.insert(p.0);
            reference |= state.dynamic.borrow().contains(&p)
                || state.cycle_pins.borrow().contains(&p)
                || state.tainted.borrow().contains(&p);
            let content = self.content_on_sheet(position(p));
            if let Content::Formula {
                array: Some((ArrayKind::Dynamic, _)),
                ..
            } = content
            {
                reference = true;
            }
            let rt = &state.rt;
            if let Some(handle) = state.content.get(rt, &p) {
                rt.set(handle, content.clone());
            }
            if let Some(handle) = state.tile.get(rt, &tile_of(p)) {
                rt.set(handle, rt.get(handle) + 1);
            }
            if let Content::Formula { .. } = content {
                state.pending.borrow_mut().push(p);
            }
            let watched = state.value.get(rt, &p);
            match (&content, watched) {
                (Content::Formula { .. }, None) => self.incr_watch(state, p),
                (Content::Formula { .. }, Some(handle)) => {
                    if !state.watched.borrow().contains_key(&handle.into()) {
                        self.incr_watch(state, p);
                    }
                }
                (_, Some(handle)) => {
                    if state.watched.borrow_mut().remove(&handle.into()).is_some() {
                        rt.unwatch(handle);
                    }
                }
                (_, None) => {}
            }
        }
        for sheet in sheets {
            let d = self.workbook.worksheets[sheet as usize].dimension();
            state.set_dimension(sheet, (d.min_row, d.max_row, d.min_column, d.max_column));
        }
        reference
    }

    /// Computes the watched formulas that changes reached and writes their
    /// values back, until no change is left. True if one of them needs the
    /// reference. After the reference ran, the members of a cycle found are
    /// pinned to what it stored instead.
    fn incr_drain(&mut self, state: &IncrState, after_reference: bool) -> bool {
        let mut reference = false;
        loop {
            let mut drained = state.rt.drain_dirtied_watched();
            let pending: Vec<Position> = state.pending.borrow_mut().drain(..).collect();
            for p in pending {
                if let Some(handle) = state.value.get(&state.rt, &p) {
                    drained.push(handle.into());
                }
            }
            if drained.is_empty() {
                break;
            }
            for handle in drained {
                let Some(p) = state.watched.borrow().get(&handle).copied() else {
                    continue;
                };
                state.newly_tainted.set(false);
                let computed = self.incr_value(position(p));
                reference |= state.newly_tainted.replace(false);
                if let Some(cycle) = state.rt.cycle_of(handle) {
                    reference = true;
                    if after_reference {
                        // Every member at once: pinning one alone would take
                        // the others off the cycle before they are pinned.
                        let members: Vec<Position> = {
                            let watched = state.watched.borrow();
                            cycle
                                .participants()
                                .iter()
                                .filter_map(|member| watched.get(member).copied())
                                .collect()
                        };
                        for member in members {
                            self.incr_pin(state, member);
                        }
                    }
                    continue;
                }
                let pinned =
                    state.dynamic.borrow().contains(&p) || state.cycle_pins.borrow().contains(&p);
                reference |= pinned || state.tainted.borrow().contains(&p);
                if !pinned {
                    self.incr_write_back(position(p), computed);
                }
            }
            reference |= self.incr_take_changes(state);
        }
        reference
    }

    /// Pins a member of a cycle: its readers take the value the reference
    /// stored.
    fn incr_pin(&self, state: &IncrState, p: Position) {
        if state.cycle_pins.borrow_mut().insert(p) {
            self.incr_refresh_content(state, p);
        }
    }

    fn incr_refresh_content(&self, state: &IncrState, p: Position) {
        if let Some(handle) = state.content.get(&state.rt, &p) {
            state.rt.set(handle, self.content_on_sheet(position(p)));
        }
    }

    /// After the reference: the pins are made again from what it stored.
    fn incr_resync(&mut self, state: &IncrState) {
        let old_pins: Vec<Position> = state.cycle_pins.borrow_mut().drain().collect();
        let old_dynamic: Vec<Position> = state.dynamic.borrow().iter().copied().collect();
        self.incr_find_dynamic(state);
        let new_dynamic: Vec<Position> = state.dynamic.borrow().iter().copied().collect();
        // The reference's circular set: what it stored as each one's own
        // circular reference, which the cycles the incremental evaluator
        // sees need not include where a dynamic anchor sits on the cycle.
        let watched: Vec<Position> = state.watched.borrow().values().copied().collect();
        for p in watched {
            let stored = match self.fetch_cell(position(p)).as_deref() {
                Some(Cell::CellFormula { v, .. } | Cell::ArrayFormula { v, .. }) => v.clone(),
                _ => continue,
            };
            if stored == self.incr_circular(position(p)).v {
                state.cycle_pins.borrow_mut().insert(p);
            }
        }
        let new_pins: Vec<Position> = state.cycle_pins.borrow().iter().copied().collect();
        for p in old_pins
            .into_iter()
            .chain(old_dynamic)
            .chain(new_dynamic)
            .chain(new_pins)
        {
            self.incr_refresh_content(state, p);
        }
        self.incr_take_changes(state);
        self.incr_drain(state, true);
    }

    fn incr_write_back(&mut self, p: CellReferenceIndex, computed: Computed) {
        let Some(cell) = self.fetch_cell(p).map(|cell| cell.into_owned()) else {
            return;
        };
        let Ok(worksheet) = self.workbook.worksheet_mut(p.sheet) else {
            return;
        };
        let written = match cell {
            Cell::CellFormula { f, s, .. } => worksheet.write_computed_cell(
                p.row,
                p.column,
                Cell::CellFormula {
                    f,
                    s,
                    v: computed.v,
                },
            ),
            Cell::ArrayFormula {
                f,
                s,
                r,
                kind: ArrayKind::Cse,
                ..
            } => {
                for ((row, column), v) in computed.spills.iter().flat_map(|s| s.iter()).cloned() {
                    let s = worksheet.get_style(row, column);
                    let spill = Cell::SpillCell {
                        a: (p.row, p.column),
                        s,
                        v,
                    };
                    let _ = worksheet.write_computed_cell(row, column, spill);
                }
                let anchor = Cell::ArrayFormula {
                    f,
                    s,
                    r,
                    kind: ArrayKind::Cse,
                    v: computed.v,
                };
                worksheet.write_computed_cell(p.row, p.column, anchor)
            }
            _ => Ok(()),
        };
        debug_assert!(written.is_ok(), "write back at {p:?}: {written:?}");
    }

    /// The invariants a recalc leaves: no change unlogged, and every watched
    /// formula whose value is not pinned stores what its node holds.
    #[cfg(debug_assertions)]
    fn incr_check(&mut self, state: &IncrState) {
        for worksheet in &self.workbook.worksheets {
            assert!(
                worksheet.sheet_data.changes().is_empty(),
                "a recalc left changes in the log"
            );
        }
        for (&sheet, &d) in state.dimensions.borrow().iter() {
            let used = self.workbook.worksheets[sheet as usize].dimension();
            assert_eq!(
                d,
                (used.min_row, used.max_row, used.min_column, used.max_column),
                "the used area of sheet {sheet} changed with no change logged"
            );
        }
        let watched: Vec<Position> = state.watched.borrow().values().copied().collect();
        for p in watched {
            if state.dynamic.borrow().contains(&p) || state.cycle_pins.borrow().contains(&p) {
                continue;
            }
            let computed = self.incr_value(position(p));
            let stored = match self.fetch_cell(position(p)).as_deref() {
                Some(Cell::CellFormula { v, .. } | Cell::ArrayFormula { v, .. }) => v.clone(),
                _ => FormulaValue::Unevaluated,
            };
            assert_eq!(
                computed.v, stored,
                "the value at {p:?} is not the one stored"
            );
        }
    }
}
