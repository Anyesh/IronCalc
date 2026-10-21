//! The value of a computed cell, to be changed without anything else about it.

use crate::types::{Cell, FormulaValue};

/// The value `v` of a formula, array formula or spill cell, to be written
/// without logging the change (see `SheetData::cell_mut_value`). Nothing else
/// about the cell can be changed through it.
#[derive(Debug)]
pub struct CellValueMut<'a>(&'a mut Cell);

impl<'a> CellValueMut<'a> {
    pub(super) fn of(cell: &'a mut Cell) -> Option<Self> {
        match cell {
            Cell::CellFormula { .. } | Cell::ArrayFormula { .. } | Cell::SpillCell { .. } => {
                Some(CellValueMut(cell))
            }
            _ => None,
        }
    }

    pub fn cell(&self) -> &Cell {
        self.0
    }

    /// Takes the value of `cell` if `cell` is the cell held here but for its
    /// value. Otherwise nothing is written and `cell` is given back.
    pub fn set_value_from(&mut self, cell: Cell) -> Result<(), Cell> {
        match (&mut *self.0, cell) {
            (
                Cell::CellFormula { f, s, v },
                Cell::CellFormula {
                    f: new_f,
                    s: new_s,
                    v: new_v,
                },
            ) if *f == new_f && *s == new_s => *v = new_v,
            (
                Cell::ArrayFormula { f, s, r, kind, v },
                Cell::ArrayFormula {
                    f: new_f,
                    s: new_s,
                    r: new_r,
                    kind: new_kind,
                    v: new_v,
                },
            ) if *f == new_f && *s == new_s && *r == new_r && *kind == new_kind => *v = new_v,
            (
                Cell::SpillCell { s, a, v },
                Cell::SpillCell {
                    s: new_s,
                    a: new_a,
                    v: new_v,
                },
            ) if *s == new_s && *a == new_a => *v = new_v,
            (_, cell) => return Err(cell),
        }
        Ok(())
    }

    /// Takes `v` as the value of a formula cell. Any other cell, an array
    /// formula included, is left as it is and `v` is given back.
    pub fn set_formula_value(&mut self, v: FormulaValue) -> Result<(), FormulaValue> {
        match &mut *self.0 {
            Cell::CellFormula { v: old, .. } => {
                *old = v;
                Ok(())
            }
            _ => Err(v),
        }
    }
}
