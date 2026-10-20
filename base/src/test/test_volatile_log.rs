#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;

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
