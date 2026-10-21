//! The read path: how a formula evaluated incrementally reads the sheet.
//! Every read goes through a node, so that the runtime knows what each value
//! depends on: `content(P)` for what a cell holds, `value(P)` for what a
//! formula stores, a tile for a stretch of a row read in one go, and
//! `dimension(s)` for the used area of a sheet.

use std::collections::HashSet;
use std::rc::Rc;

use incr_compute::{CycleStrategy, Incr, QueryOpts, Replay};

use super::nodes::{tile_of, Computed, Constant, Content, Dimension, Position};
use super::{ctx, IncrState};
use crate::calc_result::CalcResult;
use crate::evaluation::{circular_reference, key, FormulaCell, SheetRead};
use crate::expressions::token::Error;
use crate::expressions::types::CellReferenceIndex;
use crate::model::Model;
use crate::types::{ArrayKind, Cell, FormulaValue};

impl IncrState {
    /// Whether the value at `p` is the reference's: a dynamic anchor or
    /// spill, or a member of a cycle.
    pub(crate) fn is_pinned(&self, p: Position) -> bool {
        self.dynamic.borrow().contains(&p) || self.cycle_pins.borrow().contains(&p)
    }

    /// Sets the `dimension` node of a sheet, if it has one, and the value
    /// read without depending on it.
    pub(crate) fn set_dimension(&self, sheet: u32, d: Dimension) {
        if let Some(handle) = self.dimension.get(&self.rt, &sheet) {
            self.rt.set(handle, d);
            self.dimensions.borrow_mut().set(sheet, d, Some(handle));
        }
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
    tiles: HashSet<Position>,
    /// The sheets whose `dimension` it depends on.
    dimensions: HashSet<u32>,
}

/// A running compute's frame. Dropped without `close` (the compute
/// unwound), it leaves the formula's taint as it was.
struct ComputeFrame {
    state: Rc<IncrState>,
}

impl ComputeFrame {
    fn push(state: &Rc<IncrState>) -> ComputeFrame {
        state.frames.borrow_mut().push(Frame::default());
        ComputeFrame {
            state: Rc::clone(state),
        }
    }

    fn close(self, p: Position) {
        let read_taint = self
            .state
            .frames
            .borrow()
            .last()
            .is_some_and(|frame| frame.read_taint);
        let mut tainted = self.state.tainted.borrow_mut();
        if read_taint {
            if tainted.insert(p) {
                self.state.newly_tainted.set(true);
            }
        } else if !tainted.is_empty() {
            tainted.remove(&p);
        }
    }
}

impl Drop for ComputeFrame {
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

    /// What the cell at `p` reads as: `evaluate_cell` while a compute of the
    /// incremental evaluator runs.
    pub(crate) fn incr_read(&mut self, p: CellReferenceIndex) -> CalcResult {
        match self.incr_content(p) {
            Content::Empty | Content::Blank => CalcResult::EmptyCell,
            Content::Constant(constant) => self.constant_value(constant, p),
            Content::Formula {
                f, pinned: Some(v), ..
            } => {
                let state = self.incr_state();
                if state.cycle_pins.borrow().contains(&key(p)) {
                    state.mark_tainted();
                }
                self.get_cell_value(&Cell::CellFormula { f, s: 0, v: *v }, p)
            }
            Content::Formula { f, .. } => {
                let v = self.incr_value(p).v;
                self.get_cell_value(&Cell::CellFormula { f, s: 0, v }, p)
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
                // Spills are in natural order, by row and then by column.
                let written = computed.spills.as_deref().and_then(|spills| {
                    let index = spills
                        .binary_search_by_key(&(p.row, p.column), |(position, _)| *position)
                        .ok()?;
                    spills.get(index)
                });
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
        let state = self.incr_state();
        // A cell outside the used area holds nothing until the area grows
        // over it: a read of a whole column needs no node per cell.
        let (_, (min_row, max_row, min_column, max_column)) = self.incr_dimension(&state, p.sheet);
        if p.row < min_row || p.row > max_row || p.column < min_column || p.column > max_column {
            self.incr_read_dimension(p.sheet);
            return Content::Empty;
        }
        let handle = state.content_index.get_or_insert(&state.rt, key(p), || {
            state.content.get_or_insert_input(&state.rt, key(p), || {
                let _read = SheetRead::tracked();
                self.content_on_sheet(p)
            })
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
        if state.tainted.borrow().contains(&key(p)) {
            state.mark_tainted();
        }
        computed
    }

    /// `value(p)`'s node, created if there is none, without computing it.
    pub(crate) fn incr_value_node(&self, p: CellReferenceIndex) -> Incr<Computed> {
        let state = self.incr_state();
        state.value_index.get_or_insert(&state.rt, key(p), || {
            state.value.get_or_insert_query_with(
                &state.rt,
                key(p),
                move |_| ctx::with_model(|model| model.incr_compute(p)),
                QueryOpts {
                    cycle: CycleStrategy::fallback(move |_| {
                        ctx::with_model(|model| model.incr_circular(p))
                    }),
                    replay: Replay::Allowed,
                },
            )
        })
    }

    /// The compute of `value(p)`: the shipped engine's run of the formula,
    /// with its reads made through nodes, and the result planned as the
    /// shipped engine would store it.
    fn incr_compute(&mut self, p: CellReferenceIndex) -> Computed {
        let Content::Formula { f, array, .. } = self.incr_content(p) else {
            // Only a formula's value is read; a doomed frame's read of the
            // content can still find the placeholder here.
            return Computed {
                v: FormulaValue::Unevaluated,
                spills: None,
            };
        };
        let cell = FormulaCell {
            f,
            s: 0,
            array: array.map(|(kind, area)| (kind == ArrayKind::Dynamic, area)),
        };
        // As in `run_top`: a link an earlier run attached may rest on values
        // this run does not read.
        if !self.links.is_empty() {
            self.links.remove(&key(p));
        }
        let state = self.incr_state();
        let frame = ComputeFrame::push(&state);
        let result = self.compute_formula(p, f);
        frame.close(key(p));
        self.incr_computed(p, cell, &result)
    }

    /// The fallback of `value(p)` when it is found on a cycle.
    pub(super) fn incr_circular(&mut self, p: CellReferenceIndex) -> Computed {
        // `incr_compute` read `content(p)` before `value(p)` could be found
        // on a cycle.
        let read = SheetRead::tracked();
        let cell = self.fetch_cell(p).as_deref().and_then(FormulaCell::of);
        drop(read);
        match cell {
            Some(cell) => self.incr_computed(p, cell, &circular_reference(p)),
            None => Computed {
                v: FormulaValue::Unevaluated,
                spills: None,
            },
        }
    }

    fn incr_computed(
        &self,
        p: CellReferenceIndex,
        cell: FormulaCell,
        result: &CalcResult,
    ) -> Computed {
        match self.plan_result(p, cell, result) {
            Ok(plan) => {
                let (v, spills) = plan.into_values();
                Computed {
                    v: v.unwrap_or(FormulaValue::Unevaluated),
                    spills: (!spills.is_empty()).then(|| {
                        spills
                            .into_iter()
                            .map(|(row, column, v)| ((row, column), v))
                            .collect()
                    }),
                }
            }
            Err(message) => Computed {
                v: FormulaValue::new_error(Error::ERROR, String::new(), message),
                spills: None,
            },
        }
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

    pub(crate) fn incr_read_dimension(&mut self, sheet: u32) {
        let state = self.incr_state();
        let (handle, _) = self.incr_dimension(&state, sheet);
        if let Some(top) = state.frames.borrow_mut().last_mut() {
            if !top.dimensions.insert(sheet) {
                return;
            }
        }
        state.rt.get(handle);
    }

    /// The used area of a sheet, without depending on it; creates its
    /// `dimension` node.
    fn incr_dimension(&self, state: &IncrState, sheet: u32) -> (Incr<Dimension>, Dimension) {
        let known = state.dimensions.borrow().get(sheet);
        let d = match known {
            Some((d, Some(handle))) => return (handle, d),
            Some((d, None)) => d,
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
