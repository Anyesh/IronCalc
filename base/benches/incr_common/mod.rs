//! The workbooks of the T12 performance gate, shared by the criterion bench
//! and the gate harness. Every builder returns an evaluated model with the
//! incremental evaluator off; the caller switches it on.

#![allow(dead_code)]

use std::time::{Duration, Instant};

use ironcalc_base::Model;

pub const ROWS: i32 = 100_000;
pub const CHAIN: i32 = 100;

pub fn empty() -> Model<'static> {
    Model::new_empty("bench", "en", "UTC", "en").expect("an empty model")
}

/// A fresh model with the same workbook, the incremental evaluator off.
pub fn copy(model: &Model) -> Model<'static> {
    Model::from_bytes(&model.to_bytes(), "en").expect("a saved model")
}

fn set(model: &mut Model, row: i32, column: i32, input: String) {
    model
        .set_user_input(0, row, column, input)
        .expect("a valid input");
}

/// Chains: `rows` numbers in A and, in B, chains of `CHAIN` rows whose head
/// row holds only the number: B(head + 1) = A(head) + A(head + 1), then
/// B(r) = B(r - 1) + A(r) to the end of the chain. With 100k rows that is
/// 100k numbers and 99k formulas, and editing a head recomputes 99.
pub fn chains(rows: i32) -> Model<'static> {
    let mut model = empty();
    for row in 1..=rows {
        set(&mut model, row, 1, row.to_string());
        let offset = (row - 1) % CHAIN;
        if offset == 1 {
            set(&mut model, row, 2, format!("=A{}+A{row}", row - 1));
        } else if offset > 1 {
            set(&mut model, row, 2, format!("=B{}+A{row}", row - 1));
        }
    }
    model.evaluate();
    model
}

/// The head of the `k`th chain.
pub fn chain_head(k: i32) -> i32 {
    k * CHAIN + 1
}

/// Running sum: `rows` numbers in A and B1 = A1, B(r) = B(r - 1) + A(r).
pub fn running_sum(rows: i32) -> Model<'static> {
    let mut model = empty();
    for row in 1..=rows {
        set(&mut model, row, 1, row.to_string());
        let formula = if row == 1 {
            "=A1".to_string()
        } else {
            format!("=B{}+A{row}", row - 1)
        };
        set(&mut model, row, 2, formula);
    }
    model.evaluate();
    model
}

/// A single column of formulas over one number: A1 = 1, A(r) = A(r - 1) + 1.
/// Against `running_sum` it separates the memory of a formula from that of
/// a referenced constant.
pub fn formula_column(rows: i32) -> Model<'static> {
    let mut model = empty();
    set(&mut model, 1, 1, "1".to_string());
    for row in 2..=rows {
        set(&mut model, row, 1, format!("=A{}+1", row - 1));
    }
    model.evaluate();
    model
}

/// `running_sum` with a dynamic-array anchor, D1 = TRIMRANGE(A1:A100), whose
/// presence makes the first incremental evaluate a reference run.
pub fn running_sum_with_anchor(rows: i32) -> Model<'static> {
    let mut model = running_sum(rows);
    set(&mut model, 1, 4, "=TRIMRANGE(A1:A100)".to_string());
    model.evaluate();
    model
}

/// `running_sum` with a two-cell cycle that reads A1, D1 = E1 + A1 and
/// E1 = D1. Once the cycle is pinned, every edit of A1 dirties a pin and
/// falls back to the shipped evaluator.
pub fn running_sum_with_cycle(rows: i32) -> Model<'static> {
    let mut model = running_sum(rows);
    set(&mut model, 1, 4, "=E1+A1".to_string());
    set(&mut model, 1, 5, "=D1".to_string());
    model.evaluate();
    model
}

/// Writes a new number into `row` of column A, alternating between two
/// values so every edit changes the cell.
pub struct Editor {
    flip: bool,
}

impl Editor {
    pub fn new() -> Self {
        Editor { flip: false }
    }

    pub fn edit(&mut self, model: &mut Model, row: i32) {
        self.flip = !self.flip;
        let value = if self.flip { -1 } else { row };
        set(model, row, 1, value.to_string());
    }
}

pub fn quantile(mut samples: Vec<Duration>, q: f64) -> Duration {
    samples.sort();
    let index = ((samples.len() - 1) as f64 * q).round() as usize;
    samples[index]
}

pub fn median(samples: Vec<Duration>) -> Duration {
    quantile(samples, 0.5)
}

/// `warm_up` untimed steps of each side, then `runs` timed steps of `incr`
/// with `reference_runs` timed steps of `reference` spread evenly among them.
pub fn interleaved(
    warm_up: usize,
    runs: usize,
    reference_runs: usize,
    mut incr: impl FnMut() -> Duration,
    mut reference: impl FnMut() -> Duration,
) -> (Vec<Duration>, Vec<Duration>) {
    for _ in 0..warm_up {
        incr();
    }
    for _ in 0..warm_up {
        reference();
    }
    let mut incr_times = Vec::with_capacity(runs);
    let mut reference_times = Vec::with_capacity(reference_runs);
    for run in 0..runs {
        incr_times.push(incr());
        for _ in run * reference_runs / runs..(run + 1) * reference_runs / runs {
            reference_times.push(reference());
        }
    }
    (incr_times, reference_times)
}

/// One edit of `row` by `editor` and the `evaluate` after it, timed.
pub fn edit_step<'a>(
    model: &'a mut Model<'static>,
    editor: &'a mut Editor,
    row: i32,
) -> impl FnMut() -> Duration + 'a {
    move || {
        let start = Instant::now();
        editor.edit(model, row);
        model.evaluate();
        start.elapsed()
    }
}
