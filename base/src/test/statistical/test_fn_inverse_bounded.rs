#![allow(clippy::unwrap_used)]

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

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
