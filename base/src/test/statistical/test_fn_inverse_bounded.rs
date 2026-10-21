#![allow(clippy::unwrap_used)]

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::cell::CellValue;
use crate::test::util::new_empty_model;

// Evaluates on a worker thread so a non-terminating inverse fails the test
// instead of hanging it; the stuck thread dies with the test process.
fn evaluate_bounded(formula: &'static str) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut model = new_empty_model();
        model._set("A1", formula);
        model.evaluate();
        let _ = tx.send(model._get_text("A1"));
    });
    rx.recv_timeout(Duration::from_secs(20)).ok()
}

#[test]
fn test_fn_inverse_beta_family_terminates() {
    let cases = [
        "=CONFIDENCE.T(0.05, 1, 1E10)",
        "=T.INV(0.05, 1E10)",
        "=T.INV.2T(0.05, 1E10)",
        "=TINV(0.05, 1E10)",
        "=T.INV(5E-101, 1000)",
        "=F.INV(0.05, 1E10, 1)",
        "=F.INV.RT(0.95, 1E10, 1)",
        "=FINV(0.95, 1E10, 1)",
        "=BETA.INV(0.5, 1E10, 1E10)",
        "=BETAINV(0.5, 1E10, 1E10)",
    ];
    let handles: Vec<_> = cases
        .iter()
        .map(|&formula| (formula, thread::spawn(move || evaluate_bounded(formula))))
        .collect();
    let mut failures = Vec::new();
    for (formula, handle) in handles {
        match handle.join().unwrap() {
            Some(text) if text == "#NUM!" => {}
            Some(text) => failures.push(format!("{formula} gave {text}")),
            None => failures.push(format!("{formula} did not finish")),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// p * p underflows for these, which leaves statrs' starting point NaN. Expected
// values are mpmath roots of I_x(a, b) = p at 60 digits.
#[test]
fn test_fn_inverse_beta_family_tiny_probability() {
    let cases = [
        ("=BETA.INV(1E-200, 2, 2)", 5.773_502_691_896_258e-101),
        ("=BETAINV(1E-200, 2, 3, 0, 5)", 2.041_241_452_319_315e-100),
        ("=BETA.INV(1E-200, 50, 2)", 9.243_775_378_285_472e-5),
        ("=T.INV(1E-200, 5)", -1.568_392_559_099_338e40),
        ("=F.INV(1E-200, 4, 6)", 6.123_724_356_957_945e-101),
    ];
    let mut model = new_empty_model();
    for (row, (formula, _)) in cases.iter().enumerate() {
        model._set(&format!("A{}", row + 1), formula);
    }
    model.evaluate();
    let mut failures = Vec::new();
    for (row, (formula, expected)) in cases.iter().enumerate() {
        let reference = model._parse_reference(&format!("A{}", row + 1));
        let value = model
            .get_cell_value_by_index(reference.sheet, reference.row, reference.column)
            .unwrap();
        match value {
            CellValue::Number(n) if ((n - expected) / expected).abs() <= 1e-12 => {}
            other => failures.push(format!("{formula} gave {other:?}, expected {expected:e}")),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
