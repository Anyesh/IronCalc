// Edit and first-evaluate times against sheet size on Node, with the
// incremental evaluator and with the shipped one, on the gate's workbooks.
// Prints one CSV line per workload and size: medians in milliseconds of
// INCR_SWEEP_RUNS steps of each side (default 30), one of each in turn as in
// the gate, and of INCR_SWEEP_LOAD_RUNS first evaluates (default 10).
// INCR_SWEEP_ROWS is a comma-separated list of sizes. `make bench-sweep`
// builds and runs it.

import { performance } from 'node:perf_hooks';

import {
    CHAIN,
    Model,
    alternating,
    bytesOf,
    chains,
    interleaved,
    runningSum,
    timedEdit,
} from './bench_common.mjs';

const RUNS = Number(process.env.INCR_SWEEP_RUNS || 30);
const LOAD_RUNS = Number(process.env.INCR_SWEEP_LOAD_RUNS || 10);
const WARM_UP = 3;
const SIZES = (process.env.INCR_SWEEP_ROWS || '10000,30000,100000,300000,1000000')
    .split(',')
    .map(Number);

function load(bytes, incremental) {
    const model = Model.fromBytes(bytes, 'en');
    model.pauseEvaluation();
    model.setIncremental(incremental);
    return model;
}

function withMode(bytes, incremental) {
    const model = load(bytes, incremental);
    model.evaluate();
    return model;
}

function report(workload, rows, [incr, reference]) {
    console.log(`${workload},${rows},${incr.toFixed(6)},${reference.toFixed(6)}`);
}

function edit(workload, bytes, rows, row) {
    const incr = withMode(bytes, true);
    const reference = withMode(bytes, false);
    const incrEdit = alternating();
    const referenceEdit = alternating();
    report(
        workload,
        rows,
        interleaved(
            () => timedEdit(incr, row, incrEdit),
            () => timedEdit(reference, row, referenceEdit),
            RUNS,
            WARM_UP,
        ),
    );
    incr.free();
    reference.free();
}

function timedFirstEvaluate(bytes, incremental) {
    const model = load(bytes, incremental);
    const start = performance.now();
    model.evaluate();
    const time = performance.now() - start;
    model.free();
    return time;
}

console.log('workload,rows,incr_ms,reference_ms');
for (const rows of SIZES) {
    const chainBytes = bytesOf(chains(rows));
    edit('chains edit one head', chainBytes, rows, Math.floor(rows / CHAIN / 2) * CHAIN + 1);
    const sumBytes = bytesOf(runningSum(rows));
    edit('running sum edit the last row', sumBytes, rows, rows);
    edit('running sum edit the midpoint', sumBytes, rows, rows / 2);
    edit('running sum edit A1', sumBytes, rows, 1);
    report(
        'first evaluate',
        rows,
        interleaved(
            () => timedFirstEvaluate(sumBytes, true),
            () => timedFirstEvaluate(sumBytes, false),
            LOAD_RUNS,
            0,
        ),
    );
}
