#![allow(clippy::unwrap_used)]

use crate::incr::ctx;
use crate::incr::nodes::{tile_of, Constant, Content};
use crate::test::util::new_empty_model;
use crate::types::{ArrayKind, Cell, FormulaValue, SpillValue};

#[test]
fn incremental_is_off_until_switched_on() {
    let mut model = new_empty_model();
    assert!(!model.is_incremental());
    model.set_incremental(true);
    assert!(model.is_incremental());
    model.set_incremental(false);
    assert!(!model.is_incremental());
}

#[test]
fn switching_on_keeps_the_shipped_results() {
    let mut model = new_empty_model();
    model.set_incremental(true);
    model._set("A1", "2");
    model._set("A2", "=A1*21");
    model.evaluate();
    assert_eq!(model._get_text("A2"), "42");
}

#[test]
fn content_drops_the_style_and_the_formula_result() {
    let unevaluated = Cell::CellFormula {
        f: 3,
        s: 0,
        v: FormulaValue::Unevaluated,
    };
    let evaluated = Cell::CellFormula {
        f: 3,
        s: 5,
        v: FormulaValue::Number(42.0),
    };
    assert_eq!(
        Content::of(Some(&unevaluated), &[]),
        Content::of(Some(&evaluated), &[])
    );
    assert_eq!(
        Content::of(Some(&Cell::NumberCell { v: 7.0, s: 2 }), &[]),
        Content::Constant(Constant::Number(7.0))
    );
    assert_eq!(
        Content::of(Some(&Cell::EmptyCell { s: 4 }), &[]),
        Content::Empty
    );
    assert_eq!(Content::of(None, &[]), Content::Empty);
    let spill = Cell::SpillCell {
        s: 1,
        a: (1, 2),
        v: SpillValue::Number(3.0),
    };
    assert_eq!(
        Content::of(Some(&spill), &[]),
        Content::Spill { anchor: (1, 2) }
    );
}

#[test]
fn content_resolves_shared_strings() {
    let strings = ["a".to_string(), "b".to_string()];
    let cell = Cell::SharedString { si: 1, s: 0 };
    assert_eq!(
        Content::of(Some(&cell), &strings),
        Content::Constant(Constant::Text("b".to_string()))
    );
    let missing = Cell::SharedString { si: 9, s: 0 };
    assert_eq!(
        Content::of(Some(&missing), &strings),
        Content::Constant(Constant::MissingString)
    );
}

#[test]
fn content_keeps_an_anchor_and_its_area() {
    let anchor = Cell::ArrayFormula {
        f: 0,
        s: 0,
        r: (2, 3),
        kind: ArrayKind::Dynamic,
        v: FormulaValue::Number(1.0),
    };
    assert_eq!(
        Content::of(Some(&anchor), &[]),
        Content::Formula {
            f: 0,
            array: Some((ArrayKind::Dynamic, (2, 3)))
        }
    );
}

#[test]
fn tiles_are_64_by_64_blocks() {
    assert_eq!(tile_of((0, 1, 1)), tile_of((0, 63, 63)));
    assert_ne!(tile_of((0, 63, 1)), tile_of((0, 64, 1)));
    assert_ne!(tile_of((0, 1, 63)), tile_of((0, 1, 64)));
    assert_ne!(tile_of((0, 1, 1)), tile_of((1, 1, 1)));
}

#[test]
fn eval_ctx_hands_out_the_armed_model_and_restores_on_drop() {
    let mut model = new_empty_model();
    assert!(!ctx::is_armed());
    {
        let _outer = ctx::arm(&mut model);
        ctx::with_model(|outer| {
            outer._set("A1", "3");
            let _inner = ctx::arm(outer);
            ctx::with_model(|inner| inner._set("A2", "4"));
        });
        assert!(ctx::is_armed());
        ctx::with_model(|outer| assert_eq!(outer._get_text("A2"), "4"));
    }
    assert!(!ctx::is_armed());
    assert_eq!(model._get_text("A1"), "3");
}

#[test]
#[should_panic(expected = "EVAL_CTX is not armed")]
fn eval_ctx_unarmed_panics() {
    ctx::with_model(|_| ());
}
