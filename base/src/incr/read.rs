//! The read path: how a formula evaluated incrementally reads the sheet.
//! Every read goes through a node, so that the runtime knows what each value
//! depends on: `content(P)` for what a cell holds, `value(P)` for what a
//! formula stores, a tile for a stretch of a row read in one go, and
//! `dimension(s)` for the used area of a sheet.

use std::rc::Rc;

use incr_compute::{CycleStrategy, QueryOpts, Replay};

use super::nodes::{tile_of, Computed, Constant, Content, Dimension};
use super::{ctx, IncrState};
use crate::calc_result::CalcResult;
use crate::evaluation::{circular_reference, key, FormulaCell};
use crate::expressions::token::Error;
use crate::expressions::types::CellReferenceIndex;
use crate::model::Model;
use crate::types::{ArrayKind, Cell, FormulaValue};

impl Model<'_> {
    fn incr_state(&self) -> Rc<IncrState> {
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
            Content::Empty => CalcResult::EmptyCell,
            Content::Constant(constant) => self.constant_value(constant, p),
            Content::Formula {
                f, pinned: Some(v), ..
            } => self.get_cell_value(&Cell::CellFormula { f, s: 0, v }, p),
            Content::Formula { f, .. } => {
                let v = self.incr_value(p).v;
                self.get_cell_value(&Cell::CellFormula { f, s: 0, v }, p)
            }
            Content::Spill {
                anchor,
                pinned: Some(v),
            } => self.get_cell_value(&Cell::SpillCell { s: 0, a: anchor, v }, p),
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
                    None => self.stored_value(p),
                }
            }
            Content::Formula {
                array: Some((ArrayKind::Dynamic, _)),
                ..
            } => self.stored_value(p),
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
        let handle = state
            .content
            .get_or_insert_input(&state.rt, key(p), || self.content_on_sheet(p));
        state.rt.get(handle)
    }

    pub(crate) fn content_on_sheet(&self, p: CellReferenceIndex) -> Content {
        let worksheet = &self.workbook.worksheets[p.sheet as usize];
        Content::with_pins(
            self.fetch_cell(p).as_deref(),
            &self.workbook.shared_strings,
            |(row, column)| match worksheet.cell(row, column).as_deref() {
                Some(Cell::ArrayFormula { kind, .. }) => Some(kind.clone()),
                _ => None,
            },
        )
    }

    /// `value(p)`, computed if it is not clean.
    pub(crate) fn incr_value(&mut self, p: CellReferenceIndex) -> Computed {
        let state = self.incr_state();
        let handle = state.value.get_or_insert_query_with(
            &state.rt,
            key(p),
            move |_| ctx::with_model(|model| model.incr_compute(p)),
            QueryOpts {
                cycle: CycleStrategy::fallback(move |_| {
                    ctx::with_model(|model| model.incr_circular(p))
                }),
                replay: Replay::Allowed,
            },
        );
        let _armed = ctx::arm(self);
        state.rt.get(handle)
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
        let result = self.compute_formula(p, f);
        self.incr_computed(p, cell, &result)
    }

    /// The fallback of `value(p)` when it is found on a cycle.
    fn incr_circular(&mut self, p: CellReferenceIndex) -> Computed {
        let cell = self.fetch_cell(p).as_deref().and_then(FormulaCell::of);
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
        &mut self,
        sheet: u32,
        row: i32,
        first_column: i32,
        last_column: i32,
    ) {
        let state = self.incr_state();
        let (_, tile_row, first) = tile_of((sheet, row, first_column));
        let (_, _, last) = tile_of((sheet, row, last_column));
        for tile_column in first..=last {
            let handle =
                state
                    .tile
                    .get_or_insert_input(&state.rt, (sheet, tile_row, tile_column), || 0);
            state.rt.get(handle);
        }
    }

    pub(crate) fn incr_read_dimension(&mut self, sheet: u32) {
        let state = self.incr_state();
        let handle = state.dimension.get_or_insert_input(&state.rt, sheet, || {
            self.workbook
                .worksheet(sheet)
                .map(|worksheet| {
                    let d = worksheet.dimension();
                    (d.min_row, d.max_row, d.min_column, d.max_column)
                })
                .unwrap_or((1, 1, 1, 1)) as Dimension
        });
        state.rt.get(handle);
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
