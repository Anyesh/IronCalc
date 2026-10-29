// The workbooks and timing helpers shared by the T12 gate (bench_incr.mjs)
// and the size sweep (bench_sweep.mjs).

import { performance } from 'node:perf_hooks';

export const { Model, liveHeapBytes } = await import(
    `../${process.env.PKG_DIR || 'pkg-nodejs-incr-bench'}/wasm.js`
);

export const CHAIN = 100;

export function build(fill) {
    const model = new Model('bench', 'en', 'UTC', 'en');
    model.pauseEvaluation();
    fill((row, column, input) => model.setUserInput(0, row, column, input));
    model.evaluate();
    return model;
}

export function chains(rows) {
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

export function runningSum(rows, extra = () => {}) {
    return build((set) => {
        for (let row = 1; row <= rows; row++) {
            set(row, 1, String(row));
            set(row, 2, row === 1 ? '=A1' : `=B${row - 1}+A${row}`);
        }
        extra(set);
    });
}

export function formulaColumn(rows) {
    return build((set) => {
        set(1, 1, '1');
        for (let row = 2; row <= rows; row++) {
            set(row, 1, `=A${row - 1}+1`);
        }
    });
}

export const withAnchor = (set) => set(1, 4, '=TRIMRANGE(A1:A100)');
export const withCycle = (set) => {
    set(1, 4, '=E1+A1');
    set(1, 5, '=D1');
};

export function quantile(samples, q) {
    const sorted = [...samples].sort((a, b) => a - b);
    return sorted[Math.round((sorted.length - 1) * q)];
}

export const median = (samples) => quantile(samples, 0.5);

// Edits a row in column A, alternating two values so every edit it makes
// changes the cell.
export function alternating() {
    let flip = false;
    return (model, row) => {
        flip = !flip;
        model.setUserInput(0, row, 1, String(flip ? -1 : row));
    };
}

// One edit of `row` by `edit` and the evaluate after it. Milliseconds.
export function timedEdit(model, row, edit) {
    const start = performance.now();
    edit(model, row);
    model.evaluate();
    return performance.now() - start;
}


// The medians of `runs` incremental and `runs` reference steps, each step
// returning its own time, taken one of each in turn after `warmUp` of each.
export function interleaved(incrStep, referenceStep, runs, warmUp) {
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

export function bytesOf(model) {
    const bytes = model.toBytes();
    model.free();
    return bytes;
}
