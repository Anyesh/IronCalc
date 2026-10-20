//! The nodes the incremental evaluator keeps per position and per sheet.

use crate::expressions::token::Error;
use crate::types::{ArrayKind, Cell};

/// `(sheet, row, column)`.
pub(crate) type Position = (u32, i32, i32);

/// What `content(P)` holds: the cell without its style and without the value
/// a formula wrote into it. A constant's text is resolved out of the shared
/// strings, so the node never depends on their indices.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Content {
    Empty,
    Constant(Constant),
    Formula {
        f: i32,
        array: Option<(ArrayKind, (i32, i32))>,
    },
    Spill {
        anchor: (i32, i32),
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
            Some(Cell::CellFormula { f, .. }) => Content::Formula { f: *f, array: None },
            Some(Cell::ArrayFormula { f, r, kind, .. }) => Content::Formula {
                f: *f,
                array: Some((kind.clone(), *r)),
            },
            Some(Cell::SpillCell { a, .. }) => Content::Spill { anchor: *a },
        }
    }
}

/// The 64×64 block a position lies in, the key of its `tile` node.
pub(crate) fn tile_of((sheet, row, column): Position) -> Position {
    (sheet, row >> 6, column >> 6)
}
