//! Criterion timings for the rows of the T12 performance gate, each beside
//! the shipped evaluator on the same workbook. `incr_gate` checks the gate
//! itself (medians, p99, ratios, memory and scale).

mod incr_common;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use incr_common::{
    chain_head, chains, copy, running_sum, running_sum_with_anchor, running_sum_with_cycle, Editor,
    CHAIN, ROWS,
};
use ironcalc_base::{incr_probe, Model};

const MODES: [(&str, bool); 2] = [("incr", true), ("reference", false)];

fn with_mode(mut model: Model<'static>, incremental: bool) -> Model<'static> {
    model.set_incremental(incremental);
    model.evaluate();
    model
}

fn edits(c: &mut Criterion) {
    let mut group = c.benchmark_group("edit");
    group.sample_size(20);
    let (chain_book, sum_book) = (chains(ROWS), running_sum(ROWS));
    let rows = [
        ("chains/head", &chain_book, chain_head(ROWS / CHAIN / 2)),
        ("running_sum/a1", &sum_book, 1),
        ("running_sum/mid", &sum_book, ROWS / 2),
        ("running_sum/last", &sum_book, ROWS),
    ];
    for (name, model, row) in rows {
        for (mode, incremental) in MODES {
            let mut model = with_mode(copy(model), incremental);
            let mut editor = Editor::new();
            group.bench_function(format!("{name}/{mode}"), |b| {
                b.iter(|| {
                    editor.edit(&mut model, row);
                    model.evaluate();
                })
            });
        }
    }
    group.finish();
}

fn first_evaluate(c: &mut Criterion) {
    let mut group = c.benchmark_group("first_evaluate");
    group.sample_size(10);
    for (name, model) in [
        ("running_sum", running_sum(ROWS)),
        ("running_sum_with_anchor", running_sum_with_anchor(ROWS)),
    ] {
        let bytes = model.to_bytes();
        for (mode, incremental) in MODES {
            group.bench_function(format!("{name}/{mode}"), |b| {
                b.iter_batched(
                    || {
                        let mut model = Model::from_bytes(&bytes, "en").expect("a saved model");
                        model.set_incremental(incremental);
                        model
                    },
                    |mut model| {
                        model.evaluate();
                        model
                    },
                    BatchSize::PerIteration,
                )
            });
        }
    }
    group.finish();
}

fn reference_recalc(c: &mut Criterion) {
    let mut group = c.benchmark_group("reference_recalc");
    group.sample_size(20);
    let base = running_sum_with_cycle(ROWS);
    for (mode, incremental) in MODES {
        let mut model = with_mode(copy(&base), incremental);
        let mut editor = Editor::new();
        group.bench_function(format!("running_sum_with_cycle/a1/{mode}"), |b| {
            b.iter(|| {
                editor.edit(&mut model, 1);
                model.evaluate();
            })
        });
    }
    group.finish();
}

fn no_op(c: &mut Criterion) {
    let mut group = c.benchmark_group("no_op");
    for (name, model) in [("chains", chains(ROWS)), ("running_sum", running_sum(ROWS))] {
        let mut model = with_mode(model, true);
        group.bench_function(format!("{name}/evaluate"), |b| b.iter(|| model.evaluate()));
        group.bench_function(format!("{name}/fingerprint_check"), |b| {
            b.iter(|| assert!(incr_probe::fingerprint_check(&model)))
        });
    }
    group.finish();
}

criterion_group!(benches, edits, first_evaluate, reference_recalc, no_op);
criterion_main!(benches);
