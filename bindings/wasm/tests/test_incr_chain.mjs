import test from 'node:test';
import assert from 'node:assert';

const { Model } = await import(`../${process.env.PKG_DIR || 'pkg-nodejs-incr'}/wasm.js`);

// The reference's nested-function chain, 64 deep, computed first all at once:
// the incremental evaluator has to stop nesting within the default shadow
// stack, as the shipped engine does.
function chain(incremental, last) {
    const model = new Model('Workbook1', 'en', 'UTC', 'en');
    model.setIncremental(incremental);
    model.pauseEvaluation();
    for (let row = 1; row < 64; row++) {
        model.setUserInput(0, row, 2, `=IF(TRUE,SUM(1,IF(TRUE,B${row + 1}+0,0)),0)`);
    }
    model.setUserInput(0, 64, 2, '1');
    model.resumeEvaluation();
    model.evaluate();
    model.setUserInput(0, 64, 2, last);
    return Array.from({ length: 64 }, (_, i) => model.getFormattedCellValue(0, i + 1, 2));
}

test('a 64-deep chain of nested formulas gives what the shipped engine gives', () => {
    const incremental = chain(true, '2');
    assert.deepStrictEqual(incremental, chain(false, '2'));
    assert.strictEqual(incremental[0], '65');
});
