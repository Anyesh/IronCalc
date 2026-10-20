#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;

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
