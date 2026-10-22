// The T12 performance gate on Node: the wasm rows of the plan's table, each
// measured against its bound, one line each, and a nonzero exit if any row
// misses. Edit timings are medians (and the chains p99) of INCR_GATE_RUNS
// steps (default 200) against the median of as many reference steps; a first
// evaluate is the median of 20 loads. A ratio times each incremental step right
// before a reference one, so a change in machine load reaches both sides alike;
// an absolute row times the incremental model alone.
// Memory is the live heap the incremental state adds, from the heap-count
// build. `make bench-incr` builds and runs it.
//
// The workbooks are those of base/benches/incr_common: chains (100k numbers
// in A, 99k formulas in 100-row chains in B), a running sum (B1 = A1,
// B(r) = B(r-1) + A(r)), a formula column (A(r) = A(r-1) + 1), and the
// running sum with an anchor or with a cycle that reads A1.

import { performance } from 'node:perf_hooks';

const { Model, liveHeapBytes } = await import(
    `../${process.env.PKG_DIR || 'pkg-nodejs-incr-bench'}/wasm.js`
);

const ROWS = 100_000;
const SCALE_ROWS = 400_000;
const CHAIN = 100;
const RUNS = Number(process.env.INCR_GATE_RUNS || 200);
const LOAD_RUNS = 20;
const WARM_UP = 10;

function build(fill) {
    const model = new Model('bench', 'en', 'UTC', 'en');
    model.pauseEvaluation();
    fill((row, column, input) => model.setUserInput(0, row, column, input));
    model.evaluate();
    return model;
}

function chains(rows) {
    return build((set) => {
        for (let row = 1; row <= rows; row++) {
            set(row, 1, String(row));
            const offset = (row - 1) % CHAIN;
            if (offset === 1) {
                set(row, 2, `=A${row - 1}+A${row}`);
            } else if (offset > 1) {
                set(row, 2, `=B${row - 1}+A${row}`);
            }
        }
    });
}

function runningSum(rows, extra = () => {}) {
    return build((set) => {
        for (let row = 1; row <= rows; row++) {
            set(row, 1, String(row));
            set(row, 2, row === 1 ? '=A1' : `=B${row - 1}+A${row}`);
        }
        extra(set);
    });
}

function formulaColumn(rows) {
    return build((set) => {
        set(1, 1, '1');
        for (let row = 2; row <= rows; row++) {
            set(row, 1, `=A${row - 1}+1`);
        }
    });
}

const withAnchor = (set) => set(1, 4, '=TRIMRANGE(A1:A100)');
const withCycle = (set) => {
    set(1, 4, '=E1+A1');
    set(1, 5, '=D1');
};

let referenceModels = 0;

function load(bytes, incremental) {
    if (!incremental) {
        referenceModels += 1;
    }
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

function quantile(samples, q) {
    const sorted = [...samples].sort((a, b) => a - b);
    return sorted[Math.round((sorted.length - 1) * q)];
}

const median = (samples) => quantile(samples, 0.5);

// Edits a row in column A, alternating two values so every edit it makes
// changes the cell.
function alternating() {
    let flip = false;
    return (model, row) => {
        flip = !flip;
        model.setUserInput(0, row, 1, String(flip ? -1 : row));
    };
}

// One edit of `row` by `edit` and the evaluate after it. Milliseconds.
function timedEdit(model, row, edit) {
    const start = performance.now();
    edit(model, row);
    model.evaluate();
    return performance.now() - start;
}

// Runs `measure`, the samples of an absolute row, and throws if it made a
// reference model: a reference recalc between the samples would evict the
// caches they run on.
function incrementalAlone(row, measure) {
    const before = referenceModels;
    const measured = measure();
    if (referenceModels !== before) {
        throw new Error(`${row} times an incremental model alone`);
    }
    return measured;
}

function editTimes(model, row) {
    const edit = alternating();
    const times = [];
    for (let run = 0; run < WARM_UP + RUNS; run++) {
        const time = timedEdit(model, row, edit);
        if (run >= WARM_UP) {
            times.push(time);
        }
    }
    return times;
}

// The medians of `runs` incremental and `runs` reference steps, each step
// returning its own time, taken one of each in turn after `warmUp` of each.
function interleaved(incrStep, referenceStep, runs, warmUp) {
    const incr = [];
    const reference = [];
    for (let run = 0; run < warmUp + runs; run++) {
        const incrTime = incrStep();
        const referenceTime = referenceStep();
        if (run >= warmUp) {
            incr.push(incrTime);
            reference.push(referenceTime);
        }
    }
    return [median(incr), median(reference)];
}

function editMedians(bytes, row) {
    const incr = withMode(bytes, true);
    const reference = withMode(bytes, false);
    const incrEdit = alternating();
    const referenceEdit = alternating();
    const medians = interleaved(
        () => timedEdit(incr, row, incrEdit),
        () => timedEdit(reference, row, referenceEdit),
        RUNS,
        WARM_UP,
    );
    incr.free();
    reference.free();
    return medians;
}

// A load's first evaluate, the load itself untimed. Milliseconds.
function timedFirstEvaluate(bytes, incremental) {
    const model = load(bytes, incremental);
    const start = performance.now();
    model.evaluate();
    const time = performance.now() - start;
    model.free();
    return time;
}

function firstEvaluateMedians(bytes) {
    return interleaved(
        () => timedFirstEvaluate(bytes, true),
        () => timedFirstEvaluate(bytes, false),
        LOAD_RUNS,
        0,
    );
}

let failed = 0;

function check(row, measured, bound, ok) {
    if (!ok) {
        failed += 1;
    }
    console.log(`${ok ? 'pass' : 'FAIL'}  ${row}: ${measured} (bound ${bound})`);
}

const ms = (value) => `${value.toFixed(3)} ms`;

function ratio(row, incr, reference, bound) {
    const r = incr / reference;
    check(row, `${r.toFixed(2)}x (${ms(incr)} against ${ms(reference)})`, `${bound}x`, r <= bound);
}

function atMost(row, measured, bound) {
    check(row, ms(measured), ms(bound), measured <= bound);
}

function runningSumRows(label, bytes, rows) {
    let [incr, reference] = editMedians(bytes, 1);
    ratio(`${label} edit A1`, incr, reference, 1.5);
    const lastRow = `${label} edit the last row`;
    const last = incrementalAlone(lastRow, () => {
        const model = withMode(bytes, true);
        const times = editTimes(model, rows);
        model.free();
        return median(times);
    });
    atMost(lastRow, last, 0.3);
    [incr, reference] = editMedians(bytes, rows / 2);
    ratio(`${label} edit the midpoint`, incr, reference, 0.8);
    [incr, reference] = firstEvaluateMedians(bytes);
    ratio(`${label} first evaluate`, incr, reference, 3.0);
}

// Live heap the incremental state adds to an evaluated model, in bytes.
function incrBytes(bytes) {
    const model = withMode(bytes, false);
    const before = liveHeapBytes();
    model.setIncremental(true);
    model.evaluate();
    const added = liveHeapBytes() - before;
    model.free();
    return added;
}

function bytesOf(model) {
    const bytes = model.toBytes();
    model.free();
    return bytes;
}

const chainBytes = bytesOf(chains(ROWS));
const head = (ROWS / CHAIN / 2) * CHAIN + 1;
let model = withMode(chainBytes, true);
const times = incrementalAlone('chains edit one head', () => editTimes(model, head));
atMost('chains edit one head, median', median(times), 0.75);
console.log(`      chains edit one head, p99: ${ms(quantile(times, 0.99))}`);
const [noOp, checks] = incrementalAlone('chains no-op evaluate', () => {
    const noOp = [];
    const checks = [];
    for (let run = 0; run < RUNS; run++) {
        let start = performance.now();
        model.evaluate();
        noOp.push(performance.now() - start);
        start = performance.now();
        if (!model.fingerprintCheck()) {
            throw new Error('the fingerprint changed with no edit');
        }
        checks.push(performance.now() - start);
    }
    return [noOp, checks];
});
atMost('chains no-op evaluate without the fingerprint check', Math.max(0, median(noOp) - median(checks)), 0.06);
atMost('chains fingerprint check, one sheet', median(checks), 0.015);
model.free();
const reference = withMode(chainBytes, false);
console.log(`      chains edit one head, reference median: ${ms(median(editTimes(reference, head)))}`);
reference.free();

const sumBytes = bytesOf(runningSum(ROWS));
runningSumRows('running sum', sumBytes, ROWS);
const anchorBytes = bytesOf(runningSum(ROWS, withAnchor));
ratio('running sum with an anchor, first evaluate', ...firstEvaluateMedians(anchorBytes), 3.5);

const cycleBytes = bytesOf(runningSum(ROWS, withCycle));
model = withMode(cycleBytes, true);
const cycleReference = withMode(cycleBytes, false);
const before = model.referenceRuns();
// The deferred edits below go on from this editor: a fresh one could write
// the value A1 already holds, and that edit runs nothing.
const editA1 = alternating();
const referenceEdit = alternating();
const cycleMedians = interleaved(
    () => timedEdit(model, 1, editA1),
    () => timedEdit(cycleReference, 1, referenceEdit),
    RUNS,
    WARM_UP,
);
cycleReference.free();
if (model.referenceRuns() - before !== WARM_UP + RUNS) {
    throw new Error('every edit of A1 under the cycle must run the reference');
}
// A reference recalc leaves the cone it reached to the next recalc that needs
// none: this is what that recalc pays, beside the A1 row's cost.
const deferred = [];
const deferredRuns = model.referenceRuns();
const editLast = alternating();
for (let run = 0; run < 50; run++) {
    editA1(model, 1);
    model.evaluate();
    const start = performance.now();
    editLast(model, ROWS);
    model.evaluate();
    deferred.push(performance.now() - start);
}
if (model.referenceRuns() - deferredRuns !== 50) {
    throw new Error('only the edits of A1 may run the reference');
}
console.log(`      first edit of the last row after a reference recalc, median: ${ms(median(deferred))}`);
model.free();
ratio('reference recalc (an edit read by a cycle pin)', ...cycleMedians, 1.3);

const perFormula = incrBytes(bytesOf(formulaColumn(ROWS))) / (ROWS - 1);
const perConstant = (incrBytes(sumBytes) - perFormula * ROWS) / ROWS;
check('memory per formula', `${perFormula.toFixed(0)} B`, '330 B', perFormula <= 330);
check('memory per referenced constant', `${perConstant.toFixed(0)} B`, '140 B', perConstant <= 140);

runningSumRows('scale (400k running sum)', bytesOf(runningSum(SCALE_ROWS)), SCALE_ROWS);

if (failed > 0) {
    console.log(`${failed} rows missed the gate`);
    process.exit(1);
}
