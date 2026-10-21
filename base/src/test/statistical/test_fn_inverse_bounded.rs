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
        "=T.INV(5E-101, 1000)",
        // Excel's legacy FINV rejects df >= 1E10; F.INV.RT does not.
        "=FINV(0.95, 1E10, 1)",
        "=FINV(0.95, 1, 1E10)",
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

fn assert_within(cases: &[(&str, f64)], tolerance: impl Fn(f64) -> f64) {
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
            CellValue::Number(n) if (n - expected).abs() <= tolerance(*expected) => {}
            other => failures.push(format!("{formula} gave {other:?}, expected {expected:e}")),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// Above a degrees-of-freedom threshold T and F quantiles come from asymptotic
// expansions; expected values are mpmath quantiles at 50 digits.
#[test]
fn test_fn_inverse_large_degrees_of_freedom() {
    let cases = [
        ("=CONFIDENCE.T(0.05, 1, 1E10)", 1.959_963_984_777_281_4e-5),
        ("=T.INV(0.05, 1E10)", -1.644_853_627_103_849_6),
        ("=T.INV.2T(0.05, 1E10)", 1.959_963_984_777_281_4),
        ("=TINV(0.05, 1E10)", 1.959_963_984_777_281_4),
        ("=T.INV(0.025, 1E5)", -1.959_987_707_534_609_6),
        ("=T.INV(1E-20, 1E8)", -9.262_342_099_516_875),
        ("=T.INV(0.999, 3E7)", 3.090_232_577_838_802_5),
        ("=F.INV(0.05, 1E10, 1)", 0.260_317_771_563_989_8),
        ("=F.INV.RT(0.95, 1E10, 1)", 0.260_317_771_563_989_8),
        ("=F.INV(0.05, 1, 1E10)", 0.003_932_140_000_216_903_3),
        ("=F.INV(0.05, 5, 1E7)", 0.229_095_223_969_228_97),
        ("=F.INV(0.95, 30, 1E7)", 1.459_100_211_574_359_3),
        ("=F.INV(0.5, 1E7, 1E7)", 1.0),
        ("=F.INV(1E-10, 1E10, 1E10)", 0.999_872_781_274_894_2),
    ];
    assert_within(&cases, |expected| 1e-12 * expected.abs());
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
    assert_within(&cases, |expected| 1e-12 * expected.abs());
}

// Roots that AS 109's absolute stopping test cannot resolve, which statrs
// returns as about 1e-16 or 7e-16, or 3e-6 off for BETA.INV(1E-6, 0.5, 3); the
// (50, 2) and T.INV(1E-20, 5) rows are resolved by AS 109 and must stay so.
// Expected values are mpmath roots at 60 digits.
#[test]
fn test_fn_inverse_beta_family_small_probability() {
    let cases = [
        ("=BETA.INV(1E-20, 2, 2)", 5.773_502_692_007_369e-11),
        ("=BETA.INV(1E-40, 2, 2)", 5.773_502_691_896_257_5e-21),
        ("=BETA.INV(1E-100, 2, 2)", 5.773_502_691_896_258e-51),
        ("=BETA.INV(1E-160, 2, 2)", 5.773_502_691_896_257e-81),
        ("=BETA.INV(1E-6, 0.5, 3)", 2.844_444_444_445_523e-13),
        ("=BETA.INV(1E-20, 0.5, 3)", 2.844_444_444_444_444_5e-41),
        ("=BETA.INV(1E-40, 0.5, 3)", 2.844_444_444_444_444_6e-81),
        ("=BETAINV(1E-100, 0.5, 3)", 2.844_444_444_444_444_3e-201),
        ("=BETA.INV(1E-20, 50, 2)", 0.371_347_373_283_581_4),
        ("=BETA.INV(1E-40, 50, 2)", 0.146_960_262_719_674_08),
        ("=BETA.INV(1E-100, 50, 2)", 0.009_245_442_155_179_749),
        ("=BETA.INV(1E-160, 50, 2)", 0.000_583_248_411_428_402_7),
        ("=BETA.INV(0.6, 0.01, 2)", 2.415_392_203_314_029e-23),
        ("=T.INV(1E-40, 5)", -156_839_255.909_933_78),
        ("=T.INV(1E-20, 1)", -3.183_098_861_837_906_7e19),
        ("=T.INV(1E-20, 5)", -15_683.925_454_365_775),
        ("=F.INV(1E-40, 4, 6)", 6.123_724_356_957_945e-21),
    ];
    assert_within(&cases, |expected| 1e-12 * expected.abs());
}

// Roots within 1e-11 of 1, where statrs is off by a few ulps (swapped tails)
// or by 2e-10. Expected values are mpmath roots at 60 digits; one ulp below 1
// is EPSILON / 2.
#[test]
fn test_fn_inverse_beta_family_root_near_one() {
    let cases = [
        ("=BETA.INV(0.99999999, 1, 0.5)", 0.999_999_999_999_999_9),
        ("=BETA.INV(0.4, 2, 0.02)", 0.999_999_999_996_997),
    ];
    assert_within(&cases, |_| f64::EPSILON / 2.0);
}
