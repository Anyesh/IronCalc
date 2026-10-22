//! The read path: how a formula evaluated incrementally reads the sheet.
//! Every read goes through a node, so that the runtime knows what each value
//! depends on: `content(P)` for what a cell holds, `value(P)` for what a
//! formula stores, a tile for a stretch of a row read in one go, and
//! `dimension(s)` for the used area of a sheet.

use std::collections::HashSet;
use std::rc::Rc;

use incr_compute::{CycleStrategy, FxBuildHasher, Incr, QueryOpts, Replay, Runtime};

use super::nodes::{tile_of, Computed, Constant, Content, Dimension, Position};
use super::{ctx, grid, IncrState};
use crate::calc_result::CalcResult;
use crate::evaluation::{circular_reference, key, FormulaCell, SheetRead};
use crate::expressions::token::Error;
use crate::expressions::types::CellReferenceIndex;
use crate::model::Model;
use crate::types::{ArrayKind, Cell, FormulaValue};

/// The compute of `value(p)` and its options.
pub(super) fn value_query(
    p: CellReferenceIndex,
) -> (
    impl Fn(&Runtime) -> Computed + Send + Sync + 'static,
    QueryOpts<Computed>,
) {
    (
        move |_: &Runtime| ctx::with_model(|model| model.incr_compute(p)),
        QueryOpts {
            cycle: CycleStrategy::TypeFallback,
            replay: Replay::Allowed,
        },
    )
}

impl IncrState {
    /// Whether the value at `p` is the reference's: a dynamic anchor or
    /// spill, or a member of a cycle.
    #[inline]
    pub(crate) fn is_pinned(&self, p: Position) -> bool {
        let dynamic = self.dynamic.borrow();
        let pins = self.cycle_pins.borrow();
        !dynamic.is_empty() && dynamic.contains(&p) || !pins.is_empty() && pins.contains(&p)
    }

    pub(crate) fn is_tainted(&self, p: Position) -> bool {
        let tainted = self.tainted.borrow();
        !tainted.is_empty() && tainted.contains(&p)
    }

    /// Whether no formula is pinned or tainted, so none needs the reference.
    pub(crate) fn holds_none(&self) -> bool {
        self.dynamic.borrow().is_empty()
            && self.cycle_pins.borrow().is_empty()
            && self.tainted.borrow().is_empty()
    }

    /// Sets the `dimension` node of a sheet, if it has one, and the value
    /// read without depending on it. A sheet first read in a doomed frame
    /// has the value but no node, and the node is later built from it.
    pub(crate) fn set_dimension(&self, sheet: u32, d: Dimension) {
        let handle = self.dimension.get(&self.rt, &sheet);
        if let Some(handle) = handle {
            self.rt.set(handle, d);
        }
        self.dimensions.borrow_mut().set(sheet, d, handle);
    }

    /// The running compute has read a cycle pin or a tainted formula.
    fn mark_tainted(&self) {
        if let Some(top) = self.frames.borrow_mut().last_mut() {
            top.read_taint = true;
        }
    }
}

/// What a running compute has read so far.
#[derive(Default)]
pub(crate) struct Frame {
    /// Whether it read a cycle pin or a tainted formula.
    read_taint: bool,
    /// A range reads a tile once per row; one dependency on it is enough,
    /// and a dependency list a whole column long is slow to publish.
    tiles: HashSet<Position, FxBuildHasher>,
    /// The sheets whose `dimension` it depends on.
    dimensions: HashSet<u32, FxBuildHasher>,
}

/// A running compute's frame. Dropped without `close` (the compute
/// unwound), it leaves the formula's taint as it was.
struct ComputeFrame<'a> {
    state: &'a IncrState,
}

impl<'a> ComputeFrame<'a> {
    fn push(state: &'a IncrState) -> ComputeFrame<'a> {
        state.frames.borrow_mut().push(Frame::default());
        ComputeFrame { state }
    }

    fn close(self, p: Position) {
        let state = self.state;
        std::mem::forget(self);
        let read_taint = state
            .frames
            .borrow_mut()
            .pop()
            .is_some_and(|frame| frame.read_taint);
        let mut tainted = state.tainted.borrow_mut();
        if read_taint {
            if tainted.insert(p) {
                state.newly_tainted.set(true);
            }
        } else if !tainted.is_empty() {
            tainted.remove(&p);
        }
    }
}

impl Drop for ComputeFrame<'_> {
    fn drop(&mut self) {
        self.state.frames.borrow_mut().pop();
    }
}

impl Model<'_> {
    #[allow(clippy::expect_used)]
    pub(super) fn incr_state(&self) -> Rc<IncrState> {
        Rc::clone(
            self.incr
                .as_ref()
                .expect("the incremental evaluator is off"),
        )
    }

    /// The state, borrowed for as long as the model is not mutated. A compute
    /// run by an `rt.get` under the borrow cannot drop it: see `replace_incr`.
    #[allow(clippy::expect_used)]
    #[inline]
    fn incr_ref(&self) -> &IncrState {
        self.incr
            .as_deref()
            .expect("the incremental evaluator is off")
    }

    /// What the cell at `p` reads as: `evaluate_cell` while a compute of the
    /// incremental evaluator runs. A formula's reader depends on `value(p)`
    /// alone, which computes from `content(p)`.
    pub(crate) fn incr_read(&mut self, p: CellReferenceIndex) -> CalcResult {
        if self.holds_computed_formula(p) {
            let computed = self.incr_value(p);
            return match computed.read_as() {
                Some(content) => self.incr_read_content(p, content.clone()),
                None => self.formula_cell_value(&computed.v, p),
            };
        }
        let content = self.incr_content(p);
        self.incr_read_content(p, content)
    }

    /// Whether the sheet holds a formula at `p` whose readers take
    /// `value(p)`: not a dynamic anchor or a cycle pin, whose readers take
    /// the reference's value from `content(p)`.
    fn holds_computed_formula(&self, p: CellReferenceIndex) -> bool {
        // It picks the node a read depends on, not what the read gives:
        // `value(p)` computes from `content(p)`.
        let _read = SheetRead::exempt();
        matches!(
            self.fetch_cell(p).as_deref(),
            Some(
                Cell::CellFormula { .. }
                    | Cell::ArrayFormula {
                        kind: ArrayKind::Cse,
                        ..
                    }
            )
        ) && !self.incr_ref().is_pinned(key(p))
    }

    fn incr_read_content(&mut self, p: CellReferenceIndex, content: Content) -> CalcResult {
        match content {
            Content::Empty | Content::Blank => CalcResult::EmptyCell,
            Content::Constant(constant) => self.constant_value(constant, p),
            Content::Formula {
                pinned: Some(v), ..
            } => {
                let state = self.incr_state();
                if state.cycle_pins.borrow().contains(&key(p)) {
                    state.mark_tainted();
                }
                self.formula_cell_value(&v, p)
            }
            Content::Formula { .. } => {
                let v = self.incr_value(p).v;
                self.formula_cell_value(&v, p)
            }
            Content::Spill {
                anchor,
                pinned: Some(v),
            } => self.get_cell_value(
                &Cell::SpillCell {
                    s: 0,
                    a: anchor,
                    v: *v,
                },
                p,
            ),
            Content::Spill { anchor, .. } => self.incr_read_spill(p, anchor),
        }
    }

    /// A spill cell that is not pinned, which a CSE anchor writes.
    fn incr_read_spill(&mut self, p: CellReferenceIndex, anchor: (i32, i32)) -> CalcResult {
        let anchor_reference = CellReferenceIndex {
            sheet: p.sheet,
            row: anchor.0,
            column: anchor.1,
        };
        match self.incr_content(anchor_reference) {
            Content::Formula {
                array: Some((ArrayKind::Cse, _)),
                ..
            } => {
                let computed = self.incr_value(anchor_reference);
                let spills = computed.spills();
                let written = spills
                    .binary_search_by_key(&(p.row, p.column), |(position, _)| *position)
                    .ok()
                    .and_then(|index| spills.get(index));
                match written {
                    Some((_, v)) => self.get_cell_value(
                        &Cell::SpillCell {
                            s: 0,
                            a: anchor,
                            v: v.clone(),
                        },
                        p,
                    ),
                    // Outside the area the anchor writes now: what an earlier
                    // evaluation left there, which nothing writes again.
                    None => {
                        let _read = SheetRead::exempt();
                        self.stored_value(p)
                    }
                }
            }
            Content::Formula {
                array: Some((ArrayKind::Dynamic, _)),
                ..
            } => {
                let _read = SheetRead::exempt();
                self.stored_value(p)
            }
            // An orphan: its anchor is gone.
            _ => CalcResult::EmptyCell,
        }
    }

    /// What `get_cell_value` gives for a constant cell.
    fn constant_value(&self, constant: Constant, p: CellReferenceIndex) -> CalcResult {
        match constant {
            Constant::Boolean(v) => CalcResult::Boolean(v),
            Constant::Number(v) => CalcResult::Number(v),
            Constant::Text(v) => CalcResult::String(v),
            Constant::Error(ei) => {
                let message = ei.to_localized_error_string(self.language);
                CalcResult::new_error(ei, p, message)
            }
            Constant::MissingString => {
                CalcResult::new_error(Error::ERROR, p, "Invalid shared string".to_string())
            }
        }
    }

    /// `content(p)`, made from the sheet the first time it is read.
    pub(crate) fn incr_content(&mut self, p: CellReferenceIndex) -> Content {
        let state = self.incr_ref();
        // A cell outside the used area holds nothing until the area grows
        // over it: a read of a whole column needs no node per cell.
        let (_, (min_row, max_row, min_column, max_column)) = self.incr_dimension(state, p.sheet);
        if p.row < min_row || p.row > max_row || p.column < min_column || p.column > max_column {
            self.incr_read_dimension(p.sheet);
            return Content::Empty;
        }
        let handle = state
            .content
            .get_or_insert_input(&state.rt, grid(key(p)), || {
                let _read = SheetRead::tracked();
                self.content_on_sheet(p)
            });
        state.rt.get(handle)
    }

    pub(crate) fn content_on_sheet(&self, p: CellReferenceIndex) -> Content {
        let worksheet = &self.workbook.worksheets[p.sheet as usize];
        let cycle_pinned = self
            .incr
            .as_ref()
            .is_some_and(|state| state.cycle_pins.borrow().contains(&key(p)));
        let cell = self.fetch_cell(p);
        if cycle_pinned {
            if let Some(Cell::CellFormula { f, v, .. }) = cell.as_deref() {
                return Content::Formula {
                    f: *f,
                    array: None,
                    pinned: Some(Box::new(v.clone())),
                };
            }
        }
        Content::with_pins(
            cell.as_deref(),
            &self.workbook.shared_strings,
            |(row, column)| match worksheet.cell(row, column).as_deref() {
                Some(Cell::ArrayFormula { kind, .. }) => Some(kind.clone()),
                _ => None,
            },
        )
    }

    /// `value(p)`, computed if it is not clean.
    pub(crate) fn incr_value(&mut self, p: CellReferenceIndex) -> Computed {
        let handle = self.incr_value_node(p);
        self.incr_value_of(p, handle)
    }

    /// `incr_value` with `value(p)`'s node in hand.
    pub(crate) fn incr_value_of(
        &mut self,
        p: CellReferenceIndex,
        handle: Incr<Computed>,
    ) -> Computed {
        let state = self.incr_state();
        let computed = {
            let _armed = ctx::arm(self);
            state.rt.get(handle)
        };
        if state.is_tainted(key(p)) {
            state.mark_tainted();
        }
        computed
    }

    /// `value(p)`'s node, created if there is none, without computing it.
    pub(crate) fn incr_value_node(&self, p: CellReferenceIndex) -> Incr<Computed> {
        let state = self.incr_ref();
        let mut created = false;
        let handle = state
            .value
            .get_or_insert_query_lazy(&state.rt, grid(key(p)), || {
                created = true;
                value_query(p)
            });
        if created {
            state
                .value_positions
                .borrow_mut()
                .insert(handle.slot(), key(p));
        }
        handle
    }

    /// The fallback of a `value` node found on a cycle.
    pub(super) fn incr_circular_node(&mut self, node: Incr<Computed>) -> Computed {
        let (sheet, row, column) = self.incr_state().value_positions.borrow()[&node.slot()];
        self.incr_circular(CellReferenceIndex { sheet, row, column })
    }

    /// The compute of `value(p)`: the shipped engine's run of the formula,
    /// with its reads made through nodes, and the result planned as the
    /// shipped engine would store it.
    fn incr_compute(&mut self, p: CellReferenceIndex) -> Computed {
        let state = self.incr_state();
        state.computes.set(state.computes.get() + 1);
        let content = self.incr_content(p);
        let Content::Formula { f, ref array, .. } = content else {
            return Computed::new(FormulaValue::Unevaluated, Vec::new()).reading_as(content);
        };
        let cell = FormulaCell {
            f,
            s: 0,
            array: array
                .as_ref()
                .map(|(kind, area)| (*kind == ArrayKind::Dynamic, *area)),
        };
        // As in `run_top`: a link an earlier run attached may rest on values
        // this run does not read.
        if !self.links.is_empty() {
            self.links.remove(&key(p));
        }
        let frame = ComputeFrame::push(&state);
        let result = self.compute_formula(p, f);
        frame.close(key(p));
        self.incr_computed(p, cell, &result).reading_as(content)
    }

    /// The fallback of `value(p)` when it is found on a cycle.
    pub(super) fn incr_circular(&mut self, p: CellReferenceIndex) -> Computed {
        // `incr_compute` read `content(p)` before `value(p)` could be found
        // on a cycle.
        let read = SheetRead::tracked();
        let cell = self.fetch_cell(p).as_deref().and_then(FormulaCell::of);
        let content = self.content_on_sheet(p);
        drop(read);
        match cell {
            Some(cell) => self.incr_computed(p, cell, &circular_reference(p)),
            None => Computed::new(FormulaValue::Unevaluated, Vec::new()),
        }
        .reading_as(content)
    }

    /// What the formula `cell` at `p` stores for `result`, as `plan_result`
    /// plans it.
    pub(crate) fn incr_computed(
        &self,
        p: CellReferenceIndex,
        cell: FormulaCell,
        result: &CalcResult,
    ) -> Computed {
        // A plain formula's plan holds `scalar_formula_value` alone.
        let computed = if cell.array.is_none() && !matches!(result, CalcResult::Array(_)) {
            self.scalar_formula_value(p, result).map(Computed::scalar)
        } else {
            self.plan_result(p, cell, result).map(|plan| {
                let (v, spills) = plan.into_values();
                Computed::new(
                    v.unwrap_or(FormulaValue::Unevaluated),
                    spills
                        .into_iter()
                        .map(|(row, column, v)| ((row, column), v))
                        .collect(),
                )
            })
        };
        computed.unwrap_or_else(|message| {
            Computed::new(
                FormulaValue::new_error(Error::ERROR, String::new(), message),
                Vec::new(),
            )
        })
    }

    /// Depends on the tiles a read of a row from `first_column` to
    /// `last_column` covers.
    pub(crate) fn incr_read_tiles(
        &self,
        sheet: u32,
        row: i32,
        first_column: i32,
        last_column: i32,
    ) {
        let state = self.incr_state();
        let (_, tile_row, first) = tile_of((sheet, row, first_column));
        let (_, _, last) = tile_of((sheet, row, last_column));
        for tile_column in first..=last {
            let tile = (sheet, tile_row, tile_column);
            if let Some(top) = state.frames.borrow_mut().last_mut() {
                if !top.tiles.insert(tile) {
                    continue;
                }
            }
            let handle = state.tile.get_or_insert_input(&state.rt, tile, || 0);
            state.tile_reads.set(state.tile_reads.get() + 1);
            state.rt.get(handle);
        }
    }

    pub(crate) fn incr_read_dimension(&self, sheet: u32) {
        let state = self.incr_ref();
        let (handle, _) = self.incr_dimension(state, sheet);
        if let Some(top) = state.frames.borrow_mut().last_mut() {
            if !top.dimensions.insert(sheet) {
                return;
            }
        }
        state.rt.get(handle);
    }

    /// The used area of a sheet, without depending on it; creates its
    /// `dimension` node.
    #[inline]
    fn incr_dimension(&self, state: &IncrState, sheet: u32) -> (Incr<Dimension>, Dimension) {
        let known = state.dimensions.borrow().get(sheet);
        match known {
            Some((d, Some(handle))) => (handle, d),
            _ => self.incr_dimension_node(state, sheet, known.map(|(d, _)| d)),
        }
    }

    /// `incr_dimension` for a sheet whose node is not alive, from the used
    /// area `known` for it, if any.
    #[inline(never)]
    fn incr_dimension_node(
        &self,
        state: &IncrState,
        sheet: u32,
        known: Option<Dimension>,
    ) -> (Incr<Dimension>, Dimension) {
        let d = match known {
            Some(d) => d,
            None => {
                let _read = SheetRead::tracked();
                self.workbook
                    .worksheet(sheet)
                    .map(|worksheet| {
                        let d = worksheet.dimension();
                        (d.min_row, d.max_row, d.min_column, d.max_column)
                    })
                    .unwrap_or((1, 1, 1, 1)) as Dimension
            }
        };
        let handle = state.dimension.get_or_insert_input(&state.rt, sheet, || d);
        // A doomed frame's miss returns a pending handle that names no node.
        let alive = state.rt.is_alive(handle).then_some(handle);
        state.dimensions.borrow_mut().set(sheet, d, alive);
        (handle, d)
    }

    pub(crate) fn incr_read_epoch(&mut self) {
        let state = self.incr_state();
        state.rt.get(state.epoch);
    }

    /// What the cell at `p` holds when the incremental evaluator computes
    /// it, without writing anything.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn incr_cell_value(&mut self, p: CellReferenceIndex) -> CalcResult {
        let _armed = ctx::arm(self);
        ctx::with_model(|model| model.incr_read(p))
    }
}
