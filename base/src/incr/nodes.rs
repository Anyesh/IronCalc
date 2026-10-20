//! The nodes the incremental evaluator keeps per position and per sheet.

use std::sync::Arc;

use crate::expressions::token::Error;
use crate::types::{ArrayKind, Cell, FormulaValue, SpillValue};

/// `(sheet, row, column)`.
pub(crate) type Position = (u32, i32, i32);

/// What `content(P)` holds: the cell without its style and without the value
/// a formula wrote into it. A constant's text is resolved out of the shared
/// strings, so the node never depends on their indices.
///
/// A pinned position also holds the value the reference wrote there, which
/// readers take instead of computing it: dynamic anchors and their spill
/// cells always are, because their spills depend on the order of evaluation.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Content {
    Empty,
    Constant(Constant),
    Formula {
        f: i32,
        array: Option<(ArrayKind, (i32, i32))>,
        pinned: Option<FormulaValue>,
    },
    Spill {
        anchor: (i32, i32),
        pinned: Option<SpillValue>,
    },
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Constant {
    Boolean(bool),
    Number(f64),
    Text(String),
    Error(Error),
    /// A shared-string index with no string, which reads as its own error.
    MissingString,
}

impl Content {
    pub(crate) fn of(cell: Option<&Cell>, shared_strings: &[String]) -> Content {
        match cell {
            None | Some(Cell::EmptyCell { .. }) => Content::Empty,
            Some(Cell::BooleanCell { v, .. }) => Content::Constant(Constant::Boolean(*v)),
            Some(Cell::NumberCell { v, .. }) => Content::Constant(Constant::Number(*v)),
            Some(Cell::ErrorCell { ei, .. }) => Content::Constant(Constant::Error(ei.clone())),
            Some(Cell::SharedString { si, .. }) => {
                Content::Constant(match shared_strings.get(*si as usize) {
                    Some(text) => Constant::Text(text.clone()),
                    None => Constant::MissingString,
                })
            }
            Some(Cell::CellFormula { f, .. }) => Content::Formula {
                f: *f,
                array: None,
                pinned: None,
            },
            Some(Cell::ArrayFormula { f, r, kind, .. }) => Content::Formula {
                f: *f,
                array: Some((kind.clone(), *r)),
                pinned: None,
            },
            Some(Cell::SpillCell { a, .. }) => Content::Spill {
                anchor: *a,
                pinned: None,
            },
        }
    }

    /// The content of a cell with its value pinned if it is a dynamic anchor,
    /// or a spill cell of the dynamic anchor `anchor_kind` names.
    pub(crate) fn with_pins(
        cell: Option<&Cell>,
        shared_strings: &[String],
        anchor_kind: impl Fn((i32, i32)) -> Option<ArrayKind>,
    ) -> Content {
        let mut content = Content::of(cell, shared_strings);
        match (&mut content, cell) {
            (
                Content::Formula {
                    array: Some((ArrayKind::Dynamic, _)),
                    pinned,
                    ..
                },
                Some(Cell::ArrayFormula { v, .. }),
            ) => *pinned = Some(v.clone()),
            (Content::Spill { anchor, pinned }, Some(Cell::SpillCell { v, .. }))
                if anchor_kind(*anchor) == Some(ArrayKind::Dynamic) =>
            {
                *pinned = Some(v.clone());
            }
            _ => {}
        }
        content
    }
}

/// A spill cell a CSE anchor writes, by row and column, and its value.
pub(crate) type SpillWrite = ((i32, i32), SpillValue);

/// What `value(P)` holds: what the formula at P stores when it is evaluated,
/// in the cell and, for a CSE anchor, in the cells of its area.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Computed {
    pub(crate) v: FormulaValue,
    pub(crate) spills: Option<Arc<[SpillWrite]>>,
}

/// What `dimension(s)` holds: the used area of a sheet, as
/// `(min_row, max_row, min_column, max_column)`.
pub(crate) type Dimension = (i32, i32, i32, i32);

/// The 64×64 block a position lies in, the key of its `tile` node.
pub(crate) fn tile_of((sheet, row, column): Position) -> Position {
    (sheet, row >> 6, column >> 6)
}
