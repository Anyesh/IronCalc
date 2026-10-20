#![allow(clippy::unwrap_used)]

use crate::sheet_data::Change;
use crate::test::util::new_empty_model;

fn start_logging(model: &mut crate::Model) {
    model.workbook.worksheets[0].sheet_data.log_changes(true);
}

/// The positions changed since the last call, each once, in natural order.
fn changes(model: &mut crate::Model) -> Vec<Change> {
    let mut changes = model.workbook.worksheets[0].sheet_data.take_changes();
    changes.sort_by_key(|change| match *change {
        Change::Cell { row, column } => (row, column),
        Change::Row(row) => (row, 0),
    });
    changes.dedup();
    changes
}

#[test]
fn computed_values_are_written_back_unlogged() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    model._set("B1", "=A1*2");
    model._set("C1", "=SEQUENCE(2)*A1");
    model
        .set_user_array_formula(0, 1, 4, 2, 2, "=A1:B2*3")
        .unwrap();
    model._set("F1", "=1/0");
    model.evaluate();
    start_logging(&mut model);
    model._set("A1", "5");
    assert_eq!(changes(&mut model), [Change::Cell { row: 1, column: 1 }]);
    model.evaluate();
    assert_eq!(changes(&mut model), []);
    assert_eq!(model._get_text("B1"), "10");
    assert_eq!(model._get_text("C2"), "10");
    assert_eq!(model._get_text("E1"), "30");
    assert_eq!(model._get_text("D2"), "0");
    assert_eq!(model._get_text("F1"), "#DIV/0!");
}

#[test]
fn a_spill_that_changes_its_area_is_logged() {
    let mut model = new_empty_model();
    model._set("A1", "2");
    model._set("B1", "=SEQUENCE(A1)");
    model.evaluate();
    start_logging(&mut model);
    model._set("A1", "3");
    changes(&mut model);
    model.evaluate();
    // the anchor, whose area grew, and the new spill cell
    assert_eq!(
        changes(&mut model),
        [
            Change::Cell { row: 1, column: 2 },
            Change::Cell { row: 3, column: 2 },
        ]
    );
    assert_eq!(model._get_text("B3"), "3");
}
