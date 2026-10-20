#![allow(clippy::unwrap_used)]

// The `incr::miri_` set: small runs that make `EVAL_CTX` hand out the model
// in every way the read path does, for Miri to check the aliasing of the
// `&mut Model` each compute gets.

use crate::expressions::types::CellReferenceIndex;
use crate::test::util::new_empty_model;
use crate::types::FormulaValue;

fn at(model: &crate::Model, cell: &str) -> CellReferenceIndex {
    model._parse_reference(cell)
}

#[test]
fn nested_computes_each_arm_the_context() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    model._set("A2", "=A1+1");
    model._set("A3", "=A2+A1");
    model.set_incremental(true);
    let p = at(&model, "A3");
    assert_eq!(model.incr_value(p).v, FormulaValue::Number(3.0));
}

#[test]
fn a_chain_past_the_frame_budget_replays_with_the_context_armed() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    for row in 2..=40 {
        model._set(&format!("A{row}"), &format!("=A{}+1", row - 1));
    }
    model.set_incremental(true);
    let p = at(&model, "A40");
    assert_eq!(model.incr_value(p).v, FormulaValue::Number(40.0));
}

#[test]
fn a_cycle_fallback_reaches_the_model_through_the_context() {
    let mut model = new_empty_model();
    model._set("A1", "=A2");
    model._set("A2", "=A1+1");
    model.set_incremental(true);
    let p = at(&model, "A1");
    assert!(matches!(model.incr_value(p).v, FormulaValue::Error { .. }));
}

#[test]
fn the_model_can_move_between_gets() {
    let mut model = new_empty_model();
    model._set("A1", "2");
    model._set("A2", "=A1*3");
    model.set_incremental(true);
    let p = at(&model, "A2");
    assert_eq!(model.incr_value(p).v, FormulaValue::Number(6.0));
    let mut moved = Box::new(model);
    assert_eq!(moved.incr_value(p).v, FormulaValue::Number(6.0));
}
