#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

// The lockstep differential over the history and the order generators: each
// of their runs is made in an incremental model and in a reference model,
// which must agree after every step, at the normal depth and at depth 1.

use crate::expressions::utils::column_to_number;
use crate::lockstep::{Edit, Lockstep};
use crate::test::dynamic_evaluation::test_history_independence::{self as history, random_edits};
use crate::test::dynamic_evaluation::test_order_independence::{self as order, random_sheet};
use crate::test::util::new_empty_model;
use crate::Model;

const DEPTHS: [Option<usize>; 2] = [None, Some(1)];

fn two_sheets() -> Model<'static> {
    let mut model = new_empty_model();
    model.add_sheet("Sheet2").unwrap();
    model
}

fn check(seeds: u64, run: impl Fn(u64, Option<usize>) -> Result<(), String>) {
    let mut failures = Vec::new();
    for depth in DEPTHS {
        for seed in 1..=seeds {
            if let Err(failure) = run(seed, depth) {
                failures.push(format!("seed {seed}, depth {depth:?}: {failure}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} runs differ:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn the_history_generator_runs_in_lockstep() {
    check(history::SEEDS, |seed, depth| {
        let mut lockstep = Lockstep::new(two_sheets(), two_sheets(), depth)?;
        for edit in random_edits(seed) {
            lockstep.apply(&match edit {
                history::Edit::Set(sheet, row, column, input) => {
                    Edit::Input(sheet, row, column, input)
                }
                history::Edit::Clear(sheet, row, column) => {
                    Edit::Input(sheet, row, column, String::new())
                }
                history::Edit::Cse(sheet, row, column, width, height, formula) => {
                    Edit::Cse(sheet, row, column, width, height, formula)
                }
            })?;
        }
        Ok(())
    });
}

/// `A1` and the like: one column letter, then the row.
fn position(reference: &str) -> (i32, i32) {
    let (column, row) = reference.split_at(1);
    (row.parse().unwrap(), column_to_number(column).unwrap())
}

#[test]
fn the_order_generator_runs_in_lockstep() {
    check(order::SEEDS, |seed, depth| {
        let mut lockstep = Lockstep::new(new_empty_model(), new_empty_model(), depth)?;
        for (reference, input) in random_sheet(seed) {
            let (row, column) = position(&reference);
            lockstep.apply(&Edit::Input(0, row, column, input))?;
        }
        Ok(())
    });
}

#[test]
fn a_difference_is_reported_with_the_steps_that_led_to_it() {
    let mut reference = new_empty_model();
    reference._set("A1", "1");
    let mut lockstep = Lockstep::new(new_empty_model(), new_empty_model(), None).unwrap();
    lockstep
        .apply(&Edit::Input(0, 2, 1, "=A1+1".to_string()))
        .unwrap();
    let failure = match Lockstep::new(new_empty_model(), reference, None) {
        Ok(_) => panic!("the models start out different"),
        Err(failure) => failure,
    };
    assert!(failure.contains("the snapshots differ after the first evaluation"));
    assert!(
        failure.contains("reference:   0!1,1 NumberCell"),
        "{failure}"
    );
}

#[test]
fn random_runs_make_every_kind_of_edit_and_both_models_take_each() {
    let workbook = || {
        let mut model = two_sheets();
        model._set("A1", "1");
        model._set("A2", "=A1*2");
        model._set("B1", "=SUM(A1:A2)");
        model
    };
    let mut taken = std::collections::BTreeMap::new();
    for seed in 1..=8 {
        let lockstep = crate::lockstep::run(workbook(), workbook(), seed, 40, None).unwrap();
        for (edit, refused) in lockstep.steps().iter().zip(lockstep.refused()) {
            let name = format!("{edit:?}");
            let kind = name[..name.find('(').unwrap_or(name.len())].to_string();
            *taken.entry(kind).or_insert(0) += usize::from(!refused);
        }
    }
    assert_eq!(taken.len(), 13, "{taken:?}");
    assert!(taken.values().all(|&n| n > 0), "{taken:?}");
}
