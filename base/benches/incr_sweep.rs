//! Edit and first-evaluate times against sheet size, with the incremental
//! evaluator and with the shipped one, on the gate's workbooks. Prints one
//! CSV line per workload and size: medians in milliseconds of
//! `INCR_SWEEP_RUNS` incremental steps (default 30) against
//! `INCR_SWEEP_REFERENCE_RUNS` reference steps (default 30), timed together
//! as in the gate, and of `INCR_SWEEP_LOAD_RUNS` first evaluates of each
//! (default 10). `INCR_SWEEP_ROWS` is a comma-separated list of sizes.
//!
//! cargo bench -p ironcalc_base --features incr --bench incr_sweep

mod incr_common;

use std::time::{Duration, Instant};

use incr_common::{
    chain_head, chains, copy, edit_step, interleaved, median, running_sum, Editor, CHAIN,
};
use ironcalc_base::Model;

const WARM_UP: usize = 3;

fn env_count(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .map(|runs| runs.parse().unwrap_or_else(|_| panic!("{name} is a count")))
        .unwrap_or(default)
}

fn sizes() -> Vec<i32> {
    std::env::var("INCR_SWEEP_ROWS")
        .unwrap_or_else(|_| "10000,30000,100000,300000,1000000".to_string())
        .split(',')
        .map(|rows| {
            rows.trim()
                .parse()
                .expect("INCR_SWEEP_ROWS lists row counts")
        })
        .collect()
}

fn with_mode(model: &Model, incremental: bool) -> Model<'static> {
    let mut model = copy(model);
    model.set_incremental(incremental);
    model.evaluate();
    model
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

struct Sweep {
    runs: usize,
    reference_runs: usize,
    load_runs: usize,
}

impl Sweep {
    fn report(&self, workload: &str, rows: i32, incr: Vec<Duration>, reference: Vec<Duration>) {
        println!(
            "{workload},{rows},{:.6},{:.6}",
            ms(median(incr)),
            ms(median(reference))
        );
    }

    fn edit(&self, workload: &str, book: &Model, rows: i32, row: i32) {
        let mut incr_model = with_mode(book, true);
        let mut reference_model = with_mode(book, false);
        let (mut incr_editor, mut reference_editor) = (Editor::new(), Editor::new());
        let (incr, reference) = interleaved(
            WARM_UP,
            self.runs,
            self.reference_runs,
            edit_step(&mut incr_model, &mut incr_editor, row),
            edit_step(&mut reference_model, &mut reference_editor, row),
        );
        self.report(workload, rows, incr, reference);
    }

    fn first_evaluate(&self, book: &Model, rows: i32) {
        let bytes = book.to_bytes();
        let step = |incremental: bool| {
            let bytes = &bytes;
            move || {
                let mut model = Model::from_bytes(bytes, "en").expect("a saved model");
                model.set_incremental(incremental);
                let start = Instant::now();
                model.evaluate();
                start.elapsed()
            }
        };
        let runs = self.load_runs;
        let (incr, reference) = interleaved(0, runs, runs, step(true), step(false));
        self.report("first evaluate", rows, incr, reference);
    }
}

fn main() {
    let sweep = Sweep {
        runs: env_count("INCR_SWEEP_RUNS", 30),
        reference_runs: env_count("INCR_SWEEP_REFERENCE_RUNS", 30),
        load_runs: env_count("INCR_SWEEP_LOAD_RUNS", 10),
    };
    println!("workload,rows,incr_ms,reference_ms");
    for rows in sizes() {
        let book = chains(rows);
        sweep.edit(
            "chains edit one head",
            &book,
            rows,
            chain_head(rows / CHAIN / 2),
        );
        drop(book);
        let book = running_sum(rows);
        sweep.edit("running sum edit the last row", &book, rows, rows);
        sweep.edit("running sum edit the midpoint", &book, rows, rows / 2);
        sweep.edit("running sum edit A1", &book, rows, 1);
        sweep.first_evaluate(&book, rows);
    }
}
