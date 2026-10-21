import test from 'node:test';
import assert from 'node:assert';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const { lockstepXlsx } = await import(
    `../${process.env.PKG_DIR || 'pkg-nodejs-xlsx-incr'}/wasm.js`
);

// The lockstep differential over the workbooks xlsx/build.rs turns into
// tests. LOCKSTEP_SEEDS (default 1,2), LOCKSTEP_DEPTHS (default normal,1) and
// LOCKSTEP_SHARD (k/n: every nth workbook from the kth) split a run.
const xlsxDir = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'xlsx');

function folders() {
    const source = readFileSync(path.join(xlsxDir, 'build.rs'), 'utf8');
    const start = source.indexOf('const FOLDERS');
    const list = source.slice(start, source.indexOf('];', start));
    const found = [...list.matchAll(/FolderConfig\s*\{([^}]*)\}/g)].map(([, body]) => ({
        dir: body.match(/dir:\s*"([^"]+)"/)[1],
        recursive: /recursive:\s*true/.test(body),
        skip: [...(body.match(/skip:\s*&\[([^\]]*)\]/)[1].matchAll(/"([^"]+)"/g))].map((m) => m[1]),
    }));
    assert.ok(found.length > 0, 'no FOLDERS in xlsx/build.rs');
    return found;
}

function workbooks(dir, recursive, skip) {
    return readdirSync(dir)
        .sort()
        .flatMap((entry) => {
            const file = path.join(dir, entry);
            if (statSync(file).isDirectory()) {
                return recursive ? workbooks(file, recursive, skip) : [];
            }
            const stem = entry.slice(0, -'.xlsx'.length);
            const wanted = entry.endsWith('.xlsx') && !entry.startsWith('~') && !skip.includes(stem);
            return wanted ? [file] : [];
        });
}

const seeds = (process.env.LOCKSTEP_SEEDS || '1,2').split(',').map(Number);
const depths = (process.env.LOCKSTEP_DEPTHS || 'normal,1')
    .split(',')
    .map((depth) => (depth === 'normal' ? undefined : Number(depth)));
const [shard, shards] = (process.env.LOCKSTEP_SHARD || '0/1').split('/').map(Number);

const files = folders()
    .flatMap(({ dir, recursive, skip }) => workbooks(path.join(xlsxDir, dir), recursive, skip))
    .filter((_, index) => index % shards === shard);

for (const file of files) {
    test(`lockstep ${path.relative(xlsxDir, file)}`, () => {
        const bytes = readFileSync(file);
        const name = path.basename(file, '.xlsx');
        for (const depth of depths) {
            for (const seed of seeds) {
                lockstepXlsx(bytes, name, seed, depth);
            }
        }
    });
}
