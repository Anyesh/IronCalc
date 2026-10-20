#![allow(clippy::unwrap_used)]

use crate::expressions::types::CellReferenceIndex;
use crate::test::util::new_empty_model;

const A1: CellReferenceIndex = CellReferenceIndex {
    sheet: 0,
    row: 1,
    column: 1,
};

#[test]
fn a_run_of_a_formula_draws_what_its_earlier_runs_drew() {
    let mut model = new_empty_model();
    model.start_formula_run(A1);
    let first = (model.draw_random(A1), model.draw_random(A1));
    assert_ne!(first.0, first.1);
    model.start_formula_run(A1);
    assert_eq!((model.draw_random(A1), model.draw_random(A1)), first);
    // a new recalc draws afresh
    model.volatile.clear();
    model.start_formula_run(A1);
    assert_ne!(model.draw_random(A1), first.0);
}

#[test]
fn two_draws_of_one_formula_differ() {
    let mut model = new_empty_model();
    model._set("A1", "=RAND()-RAND()");
    model.evaluate();
    assert_ne!(model._get_text("A1"), "0");
}

#[test]
fn every_evaluation_draws_afresh() {
    let mut model = new_empty_model();
    model._set("A1", "=RAND()");
    model.evaluate();
    let first = model._get_text("A1");
    model.evaluate();
    assert_ne!(model._get_text("A1"), first);
}
