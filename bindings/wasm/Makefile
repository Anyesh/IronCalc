# Two wasm-pack builds from this crate:
#   pkg/       — @ironcalc/wasm       (engine only, default)
#   pkg-xlsx/  — @ironcalc/wasm-xlsx  (engine + xlsx import/export)
#
# The xlsx APIs are gated on `--features xlsx`, so the default bundle stays
# the same size. Both builds go through wasm-pack (and therefore wasm-opt).

# In some platforms, python is called python3
PYTHON := $(shell command -v python 2>/dev/null || command -v python3 2>/dev/null)

# If neither is found, fail immediately
ifeq ($(PYTHON),)
  $(error No python found. Please install python.)
endif

# Content hash of everything that feeds the wasm builds. The build targets are
# stamp files named after it, so `make wasm` / `make xlsx` are no-ops while the
# sources are unchanged.
# Requires a git checkout.
SRC_HASH := $(shell cd ../.. && git ls-files -co --exclude-standard -- bindings/wasm base xlsx Cargo.toml Cargo.lock | sort | while IFS= read -r f; do [ -f "$$f" ] && printf '%s\n' "$$f"; done | git hash-object --stdin-paths | git hash-object --stdin)

all: wasm xlsx

wasm: pkg/.built-$(SRC_HASH)

# `deps` and `types.js` are order-only (after the |): `deps` is .PHONY and
# would otherwise force the stamp to rebuild on every run, and types.js only
# changes when types.ts does, which the hash already covers.
pkg/.built-$(SRC_HASH): | deps types.js
	wasm-pack build --target web --scope ironcalc --release
	cp README.pkg.md pkg/README.md
	$(PYTHON) fix_types.py pkg
	$(PYTHON) fix_package.py pkg
	node --test tests/test_package.mjs
	rm -f pkg/.built-*
	touch $@

xlsx: pkg-xlsx/.built-$(SRC_HASH)

pkg-xlsx/.built-$(SRC_HASH): | deps types.js
	wasm-pack build --target web --scope ironcalc --release --out-dir pkg-xlsx -- --features xlsx
	cp README.pkg-xlsx.md pkg-xlsx/README.md
	$(PYTHON) fix_types.py pkg-xlsx
	$(PYTHON) fix_package.py pkg-xlsx @ironcalc/wasm-xlsx
	PKG_DIR=pkg-xlsx node --test tests/test_package.mjs
	rm -f pkg-xlsx/.built-*
	touch $@

# types.ts compiles to types.js, which fix_types.py prepends to the pkg's
# wasm.js. One shared target so parallel `make -j` builds don't race on it.
types.js: types.ts | deps
	pnpm exec tsc types.ts --target esnext --module esnext

# Installs the pinned JS tooling (typescript, typedoc) from pnpm-lock.yaml
deps:
	pnpm install --frozen-lockfile

# The nodejs test builds go to their own out-dirs so they don't clobber the
# publishable web builds in pkg/ and pkg-xlsx/. They use --dev (no
# optimizations, no wasm-opt) since they only exist to run the tests.
tests:
	wasm-pack build --dev --target nodejs --out-dir pkg-nodejs && node tests/test.mjs && node tests/test_cf.mjs
	wasm-pack build --dev --target nodejs --out-dir pkg-nodejs-xlsx -- --features xlsx && node tests/test_xlsx.mjs

# The same tests against builds with the incremental evaluator, plus its own:
# the 64-deep nested chain in a debug and a release build, and the lockstep
# over the workbook set (see test_incr.mjs for splitting it). The xlsx build is
# a release one with a mocked clock: the lockstep's two models must read the
# same NOW(), and unoptimized it takes hours.
tests-incr:
	wasm-pack build --dev --target nodejs --out-dir pkg-nodejs-incr -- --features incr
	PKG_DIR=pkg-nodejs-incr node tests/test.mjs && PKG_DIR=pkg-nodejs-incr node tests/test_cf.mjs && PKG_DIR=pkg-nodejs-incr node tests/test_incr_chain.mjs
	wasm-pack build --release --target nodejs --out-dir pkg-nodejs-xlsx-incr -- --features xlsx,incr,mock_time
	PKG_DIR=pkg-nodejs-xlsx-incr node tests/test_xlsx.mjs && PKG_DIR=pkg-nodejs-xlsx-incr node tests/test_incr_chain.mjs && PKG_DIR=pkg-nodejs-xlsx-incr node tests/test_incr.mjs

# The T12 performance gate on Node (tests/bench_incr.mjs), on a release build
# with the incremental evaluator and the live-heap counter.
bench-incr:
	wasm-pack build --release --target nodejs --out-dir pkg-nodejs-incr-bench -- --features incr,heap-count
	PKG_DIR=pkg-nodejs-incr-bench node tests/bench_incr.mjs

# Edit and first-evaluate times against sheet size, as CSV (tests/bench_sweep.mjs).
bench-sweep:
	wasm-pack build --release --target nodejs --out-dir pkg-nodejs-incr-bench -- --features incr,heap-count
	PKG_DIR=pkg-nodejs-incr-bench node tests/bench_sweep.mjs

lint:
	cargo check
	cargo fmt -- --check
	cargo clippy --all-targets --all-features -- -D warnings

clean:
	cargo clean
	rm -rf pkg pkg-xlsx pkg-nodejs pkg-nodejs-xlsx pkg-nodejs-incr pkg-nodejs-xlsx-incr pkg-nodejs-incr-bench
	rm -f types.js

.PHONY: all wasm xlsx deps lint clean tests tests-incr bench-incr bench-sweep
