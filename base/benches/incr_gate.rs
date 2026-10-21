//! The T12 performance gate, natively: every row of the plan's table
//! measured against its bound, one line each, and a nonzero exit if any
//! row misses. Timings are medians (and the chains p99) of `INCR_GATE_RUNS`
//! incremental steps (default 200) against the median of 50 reference
//! steps; memory is the live heap the incremental state adds, from a
//! counting allocator.
//!
//! cargo bench -p ironcalc_base --features incr --bench incr_gate

mod incr_common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::process::ExitCode;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use incr_common::{
    chain_head, chains, copy, formula_column, running_sum, running_sum_with_anchor,
    running_sum_with_cycle, Editor, CHAIN, ROWS,
};
use ironcalc_base::{incr_probe, Model};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(
            new_size as isize - layout.size() as isize,
            Ordering::Relaxed,
        );
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const REFERENCE_RUNS: usize = 50;
const WARM_UP: usize = 10;
const SCALE_ROWS: i32 = 400_000;

struct Gate {
    failed: usize,
}

impl Gate {
    fn check(&mut self, row: &str, measured: String, bound: String, ok: bool) {
        if !ok {
            self.failed += 1;
        }
        let verdict = if ok { "pass" } else { "FAIL" };
        println!("{verdict}  {row}: {measured} (bound {bound})");
    }

    fn ratio(&mut self, row: &str, incr: Duration, reference: Duration, bound: f64) {
        let ratio = incr.as_secs_f64() / reference.as_secs_f64();
        self.check(
            row,
            format!("{ratio:.2}x ({incr:.3?} against {reference:.3?})"),
            format!("{bound}x"),
            ratio <= bound,
        );
    }

    fn at_most(&mut self, row: &str, measured: Duration, bound: Duration) {
        self.check(
            row,
            format!("{measured:.3?}"),
            format!("{bound:.3?}"),
            measured <= bound,
        );
    }
}

fn incr_runs() -> usize {
    std::env::var("INCR_GATE_RUNS")
        .ok()
        .map(|runs| runs.parse().expect("INCR_GATE_RUNS is a count"))
        .unwrap_or(200)
}

fn quantile(mut samples: Vec<Duration>, q: f64) -> Duration {
    samples.sort();
    let index = ((samples.len() - 1) as f64 * q).round() as usize;
    samples[index]
}

fn median(samples: Vec<Duration>) -> Duration {
    quantile(samples, 0.5)
}

fn with_mode(model: &Model, incremental: bool) -> Model<'static> {
    let mut model = copy(model);
    model.set_incremental(incremental);
    model.evaluate();
    model
}

/// One edit of `row` and the `evaluate` after it, `runs` times after a
/// warm-up.
fn edit_times(model: &mut Model, row: i32, runs: usize) -> Vec<Duration> {
    let mut editor = Editor::new();
    let mut times = Vec::with_capacity(runs);
    for run in 0..WARM_UP + runs {
        let start = Instant::now();
        editor.edit(model, row);
        model.evaluate();
        if run >= WARM_UP {
            times.push(start.elapsed());
        }
    }
    times
}

fn edit_medians(book: &Model, row: i32) -> (Duration, Duration) {
    let incr = median(edit_times(&mut with_mode(book, true), row, incr_runs()));
    let reference = median(edit_times(&mut with_mode(book, false), row, REFERENCE_RUNS));
    (incr, reference)
}

/// The first `evaluate` of a model loaded from bytes.
fn first_evaluate(book: &Model, incremental: bool, runs: usize) -> Duration {
    let bytes = book.to_bytes();
    let times = (0..runs)
        .map(|_| {
            let mut model = Model::from_bytes(&bytes, "en").expect("a saved model");
            model.set_incremental(incremental);
            let start = Instant::now();
            model.evaluate();
            start.elapsed()
        })
        .collect();
    median(times)
}

fn running_sum_rows(gate: &mut Gate, label: &str, book: &Model, rows: i32, load_runs: usize) {
    let (incr, reference) = edit_medians(book, 1);
    gate.ratio(&format!("{label} edit A1"), incr, reference, 1.5);
    let (incr, _) = edit_medians(book, rows);
    gate.at_most(
        &format!("{label} edit the last row"),
        incr,
        Duration::from_micros(100),
    );
    let (incr, reference) = edit_medians(book, rows / 2);
    gate.ratio(&format!("{label} edit the midpoint"), incr, reference, 0.8);
    let incr = first_evaluate(book, true, load_runs);
    let reference = first_evaluate(book, false, load_runs);
    gate.ratio(&format!("{label} first evaluate"), incr, reference, 3.0);
}

/// Live heap the incremental state adds to an evaluated model, in bytes.
fn incr_bytes(book: &Model) -> f64 {
    let mut model = copy(book);
    model.evaluate();
    let before = LIVE.load(Ordering::Relaxed);
    model.set_incremental(true);
    model.evaluate();
    let after = LIVE.load(Ordering::Relaxed);
    drop(model);
    (after - before) as f64
}

fn main() -> ExitCode {
    let mut gate = Gate { failed: 0 };

    let book = chains(ROWS);
    let head = chain_head(ROWS / CHAIN / 2);
    let times = edit_times(&mut with_mode(&book, true), head, incr_runs());
    let p99 = quantile(times.clone(), 0.99);
    gate.at_most(
        "chains edit one head, median",
        median(times),
        Duration::from_micros(250),
    );
    gate.at_most("chains edit one head, p99", p99, Duration::from_micros(500));
    let reference = median(edit_times(
        &mut with_mode(&book, false),
        head,
        REFERENCE_RUNS,
    ));
    println!("      chains edit one head, reference median: {reference:.3?}");
    let mut model = with_mode(&book, true);
    let no_op = median(
        (0..incr_runs())
            .map(|_| {
                let start = Instant::now();
                model.evaluate();
                start.elapsed()
            })
            .collect(),
    );
    let check = median(
        (0..incr_runs())
            .map(|_| {
                let start = Instant::now();
                assert!(incr_probe::fingerprint_check(&model), "nothing changed");
                start.elapsed()
            })
            .collect(),
    );
    gate.at_most(
        "chains no-op evaluate without the fingerprint check",
        no_op.saturating_sub(check),
        Duration::from_micros(20),
    );
    gate.at_most(
        "chains fingerprint check, one sheet",
        check,
        Duration::from_micros(5),
    );
    drop((book, model));

    let book = running_sum(ROWS);
    running_sum_rows(&mut gate, "running sum", &book, ROWS, 20);
    let incr = first_evaluate(&running_sum_with_anchor(ROWS), true, 20);
    let reference = first_evaluate(&running_sum_with_anchor(ROWS), false, 20);
    gate.ratio(
        "running sum with an anchor, first evaluate",
        incr,
        reference,
        3.5,
    );

    let cycle = running_sum_with_cycle(ROWS);
    let mut model = with_mode(&cycle, true);
    let before = incr_probe::counts(&model)
        .expect("incremental")
        .reference_runs;
    let incr = median(edit_times(&mut model, 1, incr_runs()));
    let runs = incr_probe::counts(&model)
        .expect("incremental")
        .reference_runs
        - before;
    assert_eq!(
        runs as usize,
        WARM_UP + incr_runs(),
        "every edit of A1 runs the reference"
    );
    let reference = median(edit_times(&mut with_mode(&cycle, false), 1, REFERENCE_RUNS));
    gate.ratio(
        "reference recalc (an edit read by a cycle pin)",
        incr,
        reference,
        1.3,
    );
    // A reference recalc leaves the cone it reached to the next recalc that
    // needs none: this is what that recalc pays, beside the A1 row's cost.
    let mut editor = Editor::new();
    let mut after = Editor::new();
    let before = incr_probe::counts(&model)
        .expect("incremental")
        .reference_runs;
    let deferred = median(
        (0..REFERENCE_RUNS)
            .map(|_| {
                editor.edit(&mut model, 1);
                model.evaluate();
                let start = Instant::now();
                after.edit(&mut model, ROWS);
                model.evaluate();
                start.elapsed()
            })
            .collect(),
    );
    assert_eq!(
        (incr_probe::counts(&model)
            .expect("incremental")
            .reference_runs
            - before) as usize,
        REFERENCE_RUNS,
        "only the edits of A1 may run the reference"
    );
    println!("      first edit of the last row after a reference recalc, median: {deferred:.3?}");
    drop((cycle, model));

    let column = incr_bytes(&formula_column(ROWS));
    let sum = incr_bytes(&book);
    let per_formula = column / f64::from(ROWS - 1);
    let per_constant = (sum - per_formula * f64::from(ROWS)) / f64::from(ROWS);
    gate.check(
        "memory per formula",
        format!("{per_formula:.0} B"),
        "400 B".to_string(),
        per_formula <= 400.0,
    );
    gate.check(
        "memory per referenced constant",
        format!("{per_constant:.0} B"),
        "170 B".to_string(),
        per_constant <= 170.0,
    );
    drop(book);

    let book = running_sum(SCALE_ROWS);
    running_sum_rows(&mut gate, "scale (400k running sum)", &book, SCALE_ROWS, 5);

    if gate.failed == 0 {
        ExitCode::SUCCESS
    } else {
        println!("{} rows missed the gate", gate.failed);
        ExitCode::FAILURE
    }
}
