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

#[cfg(debug_assertions)]
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeSet;
#[cfg(debug_assertions)]
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use super::nodes::{tile_of, Computed, Content, Dimension, Position};
use super::IncrState;
use crate::expressions::types::CellReferenceIndex;
use crate::model::Model;
use crate::sheet_data::Change;
#[cfg(debug_assertions)]
use crate::types::FormulaValue;
use crate::types::{ArrayKind, Cell, MergedCell, SheetState, Worksheet};

/// What the state was built from besides the cells, as far as it is cheap
/// to compare on every recalc. When it no longer matches, the state is
/// rebuilt. Defined names, tables, hidden rows and rewrites of a formula
/// list change only through mutators that call `Model::metadata_edited`.
/// Appending to a formula list is not flagged: the indices cells hold stay
/// valid, and each cell that takes a new formula logs its change. A test
/// build checks both against `Untracked`, whose formula check compares only
/// each list's old prefix.
pub(crate) struct Fingerprint {
    workbook_name: String,
    tz: String,
    locale_id: String,
    locale: usize,
    language: usize,
    sheets: Vec<SheetFingerprint>,
    #[cfg(debug_assertions)]
    untracked: Untracked,
}

struct SheetFingerprint {
    name: String,
    sheet_id: u32,
    state: SheetState,
    merged_cells: Vec<MergedCell>,
}

impl SheetFingerprint {
    fn of(worksheet: &Worksheet) -> SheetFingerprint {
        SheetFingerprint {
            name: worksheet.name.clone(),
            sheet_id: worksheet.sheet_id,
            state: worksheet.state.clone(),
            merged_cells: worksheet.merged_cells.clone(),
        }
    }

    fn matches(&self, worksheet: &Worksheet) -> bool {
        self.name == worksheet.name
            && self.sheet_id == worksheet.sheet_id
            && self.state == worksheet.state
            && self.merged_cells == worksheet.merged_cells
    }
}

/// A hash of what `Fingerprint` leaves to `metadata_edited`.
#[cfg(debug_assertions)]
struct Untracked {
    metadata: u64,
    /// Per sheet, how many formulas it had and a hash of them: a sheet's
    /// formula list only grows while the indices into it stay valid.
    formulas: Vec<(usize, u64)>,
}

#[cfg(debug_assertions)]
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
        Fingerprint {
            workbook_name: workbook.name.clone(),
            tz: workbook.settings.tz.clone(),
            locale_id: workbook.settings.locale.clone(),
            locale: self.locale as *const _ as usize,
            language: self.language as *const _ as usize,
            sheets: workbook
                .worksheets
                .iter()
                .map(SheetFingerprint::of)
                .collect(),
            #[cfg(debug_assertions)]
            untracked: self.untracked(),
        }
    }

    fn fingerprint_matches(&self, built: &Fingerprint) -> bool {
        let workbook = &self.workbook;
        let matches = workbook.name == built.workbook_name
            && workbook.settings.tz == built.tz
            && workbook.settings.locale == built.locale_id
            && self.locale as *const _ as usize == built.locale
            && self.language as *const _ as usize == built.language
            && workbook.worksheets.len() == built.sheets.len()
            && workbook
                .worksheets
                .iter()
                .zip(&built.sheets)
                .all(|(worksheet, sheet)| sheet.matches(worksheet));
        #[cfg(debug_assertions)]
        assert!(
            !matches || self.untracked_matches(&built.untracked),
            "metadata changed without metadata_edited"
        );
        matches
    }

    #[cfg(debug_assertions)]
    fn untracked(&self) -> Untracked {
        let workbook = &self.workbook;
        let mut hasher = DefaultHasher::new();
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
            for row in worksheet.rows.iter().filter(|row| row.hidden) {
                row.r.hash(&mut hasher);
            }
        }
        Untracked {
            metadata: hasher.finish(),
            formulas: workbook
                .worksheets
                .iter()
                .map(|w| (w.shared_formulas.len(), hash_of(&w.shared_formulas)))
                .collect(),
        }
    }

    #[cfg(debug_assertions)]
    fn untracked_matches(&self, built: &Untracked) -> bool {
        self.untracked().metadata == built.metadata
            && self.workbook.worksheets.len() == built.formulas.len()
            && self.workbook.worksheets.iter().zip(&built.formulas).all(
                |(worksheet, &(len, hash))| {
                    worksheet.shared_formulas.len() >= len
                        && hash_of(&worksheet.shared_formulas[..len]) == hash
                },
            )
    }

    /// The fingerprint check a no-op `evaluate` makes against the stored
    /// fingerprint.
    pub(crate) fn incr_fingerprint_check(&self) -> bool {
        let state = self.incr_state();
        !state.metadata_edited.get()
            && state
                .fingerprint
                .borrow()
                .as_ref()
                .is_some_and(|built| self.fingerprint_matches(built))
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
        let current = !state.metadata_edited.get()
            && state
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
        state.used_area_scans.set(old.used_area_scans.get());
        self.incr = Some(Rc::clone(&state));
        *state.fingerprint.borrow_mut() = Some(self.fingerprint());
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
        for &p in &touched {
            reference |= state.is_pinned(p) || state.tainted.borrow().contains(&p);
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
        let known: Vec<(u32, Dimension)> = state
            .dimensions
            .borrow()
            .iter()
            .map(|(&sheet, &d)| (sheet, d))
            .collect();
        for (sheet, d) in known {
            let edited = touched
                .range((sheet, i32::MIN, i32::MIN)..=(sheet, i32::MAX, i32::MAX))
                .map(|&(_, row, column)| (row, column));
            let after = self.used_area_after(state, sheet, d, edited);
            if after != d {
                state.set_dimension(sheet, after);
            }
        }
        reference
    }

    /// The used area of `sheet` after edits of `cells`, from `known`, the
    /// area before them. A cell set outside it grows it; only a cell removed
    /// on its edge, or an edit of a sheet `dimension` reported as A1 alone
    /// (which it also reports for an empty sheet), walks every cell again.
    fn used_area_after(
        &self,
        state: &IncrState,
        sheet: u32,
        known: Dimension,
        cells: impl Iterator<Item = (i32, i32)>,
    ) -> Dimension {
        let worksheet = &self.workbook.worksheets[sheet as usize];
        let (min_row, max_row, min_column, max_column) = known;
        let mut area = known;
        for (row, column) in cells {
            let present = worksheet.sheet_data.cell(row, column).is_some();
            if present && known != (1, 1, 1, 1) {
                area = (
                    area.0.min(row),
                    area.1.max(row),
                    area.2.min(column),
                    area.3.max(column),
                );
            } else if present
                || row == min_row
                || row == max_row
                || column == min_column
                || column == max_column
            {
                state.used_area_scans.set(state.used_area_scans.get() + 1);
                let d = worksheet.dimension();
                return (d.min_row, d.max_row, d.min_column, d.max_column);
            }
        }
        area
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
                let pinned = state.is_pinned(p);
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
        // At most one reference per recalc: what these find needing it, a
        // formula tainted just now, waits for the next recalc.
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
            if state.is_pinned(p) {
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
