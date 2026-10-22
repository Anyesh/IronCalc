//! The T12 performance gate, natively: every row of the plan's table
//! measured against its bound, one line each, and a nonzero exit if any
//! row misses. Timings are medians (and the chains p99) of `INCR_GATE_RUNS`
//! incremental steps (default 200) against the median of 50 reference
//! steps; memory is the live heap the incremental state adds, from a
//! counting allocator. A ratio row times its two sides together, the
//! reference steps spread evenly among the incremental ones, so a change in
//! machine load lands on both sides alike; an absolute row times the
//! incremental model alone. The chains edit also has a user-instruction
//! count per step, from a perf counter, which machine load does not move;
//! where the counter can't be opened (not Linux, or `perf_event_paranoid`
//! above 2) the row is skipped. Its bound is relative to a recorded count,
//! so a change that moves the count on purpose records the new one.
//! `INCR_GATE_CHAINS_ONLY` set stops after the chains edit rows.
//!
//! cargo bench -p ironcalc_base --features incr --bench incr_gate

mod incr_common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::process::ExitCode;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use incr_common::{
    chain_head, chains, copy, formula_column, running_sum, running_sum_with_anchor,
    running_sum_with_cycle, Editor, CHAIN, ROWS,
};
use ironcalc_base::{incr_probe, Model};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static REFERENCE_MODELS: AtomicUsize = AtomicUsize::new(0);

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
/// User instructions per chains edit, recorded with incr at
/// t12-base-0c85491.
const CHAINS_EDIT_INSTRUCTIONS: f64 = 344_832.0;
const CHAINS_EDIT_INSTRUCTIONS_SLACK: f64 = 1.05;

struct Gate {
    failed: usize,
}

impl Gate {
    fn exit_code(&self) -> ExitCode {
        if self.failed == 0 {
            ExitCode::SUCCESS
        } else {
            println!("{} rows missed the gate", self.failed);
            ExitCode::FAILURE
        }
    }

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

fn set_mode(model: &mut Model, incremental: bool) {
    if !incremental {
        REFERENCE_MODELS.fetch_add(1, Ordering::Relaxed);
    }
    model.set_incremental(incremental);
}

fn with_mode(model: &Model, incremental: bool) -> Model<'static> {
    let mut model = copy(model);
    set_mode(&mut model, incremental);
    model.evaluate();
    model
}

/// Runs `measure`, the samples of an absolute row, and panics if it made a
/// reference model: a reference recalc between the samples would evict the
/// caches they run on.
fn incremental_alone<T>(row: &str, measure: impl FnOnce() -> T) -> T {
    let before = REFERENCE_MODELS.load(Ordering::Relaxed);
    let measured = measure();
    assert_eq!(
        REFERENCE_MODELS.load(Ordering::Relaxed),
        before,
        "{row} times an incremental model alone"
    );
    measured
}

/// `warm_up` untimed steps of each side, then `runs` timed steps of `incr`
/// with `reference_runs` timed steps of `reference` spread evenly among them.
fn interleaved(
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
fn edit_step<'a>(
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

/// `edit_step`, `runs` times after a warm-up.
fn edit_times(
    model: &mut Model<'static>,
    editor: &mut Editor,
    row: i32,
    runs: usize,
) -> Vec<Duration> {
    let mut step = edit_step(model, editor, row);
    for _ in 0..WARM_UP {
        step();
    }
    (0..runs).map(|_| step()).collect()
}

/// User instructions per `edit_step` over `runs` steps after a warm-up.
fn edit_instructions(
    model: &mut Model<'static>,
    editor: &mut Editor,
    row: i32,
    runs: usize,
) -> Result<f64, String> {
    let counter = instructions::Counter::open()?;
    let mut step = edit_step(model, editor, row);
    for _ in 0..WARM_UP {
        step();
    }
    counter.start();
    for _ in 0..runs {
        step();
    }
    Ok(counter.stop()? as f64 / runs as f64)
}

fn edit_medians(book: &Model, row: i32) -> (Duration, Duration) {
    let mut incr_model = with_mode(book, true);
    let mut reference_model = with_mode(book, false);
    let (mut incr_editor, mut reference_editor) = (Editor::new(), Editor::new());
    let (incr, reference) = interleaved(
        WARM_UP,
        incr_runs(),
        REFERENCE_RUNS,
        edit_step(&mut incr_model, &mut incr_editor, row),
        edit_step(&mut reference_model, &mut reference_editor, row),
    );
    (median(incr), median(reference))
}

/// The first `evaluate` of a model loaded from `bytes`, timed without the load.
fn first_evaluate_step(bytes: &[u8], incremental: bool) -> impl FnMut() -> Duration + '_ {
    move || {
        let mut model = Model::from_bytes(bytes, "en").expect("a saved model");
        set_mode(&mut model, incremental);
        let start = Instant::now();
        model.evaluate();
        start.elapsed()
    }
}

fn first_evaluate_medians(book: &Model, runs: usize) -> (Duration, Duration) {
    let bytes = book.to_bytes();
    let (incr, reference) = interleaved(
        0,
        runs,
        runs,
        first_evaluate_step(&bytes, true),
        first_evaluate_step(&bytes, false),
    );
    (median(incr), median(reference))
}

fn running_sum_rows(gate: &mut Gate, label: &str, book: &Model, rows: i32, load_runs: usize) {
    let (incr, reference) = edit_medians(book, 1);
    gate.ratio(&format!("{label} edit A1"), incr, reference, 1.5);
    let row = format!("{label} edit the last row");
    let incr = incremental_alone(&row, || {
        median(edit_times(
            &mut with_mode(book, true),
            &mut Editor::new(),
            rows,
            incr_runs(),
        ))
    });
    gate.at_most(&row, incr, Duration::from_micros(100));
    let (incr, reference) = edit_medians(book, rows / 2);
    gate.ratio(&format!("{label} edit the midpoint"), incr, reference, 0.8);
    let (incr, reference) = first_evaluate_medians(book, load_runs);
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
    let times = incremental_alone("chains edit one head", || {
        edit_times(
            &mut with_mode(&book, true),
            &mut Editor::new(),
            head,
            incr_runs(),
        )
    });
    let p99 = quantile(times.clone(), 0.99);
    gate.at_most(
        "chains edit one head, median",
        median(times),
        Duration::from_micros(250),
    );
    gate.at_most("chains edit one head, p99", p99, Duration::from_micros(500));
    let row = "chains edit one head, instructions";
    match incremental_alone(row, || {
        edit_instructions(
            &mut with_mode(&book, true),
            &mut Editor::new(),
            head,
            incr_runs(),
        )
    }) {
        Ok(per_edit) => gate.check(
            row,
            format!("{per_edit:.0} per edit"),
            format!("{CHAINS_EDIT_INSTRUCTIONS_SLACK}x the recorded {CHAINS_EDIT_INSTRUCTIONS:.0}"),
            per_edit <= CHAINS_EDIT_INSTRUCTIONS * CHAINS_EDIT_INSTRUCTIONS_SLACK,
        ),
        Err(why) => println!("skip  {row}: {why}"),
    }
    if std::env::var_os("INCR_GATE_CHAINS_ONLY").is_some() {
        return gate.exit_code();
    }
    let reference = median(edit_times(
        &mut with_mode(&book, false),
        &mut Editor::new(),
        head,
        REFERENCE_RUNS,
    ));
    println!("      chains edit one head, reference median: {reference:.3?}");
    let mut model = with_mode(&book, true);
    let (no_op, check) = incremental_alone("chains no-op evaluate", || {
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
        (no_op, check)
    });
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
    let (incr, reference) = first_evaluate_medians(&running_sum_with_anchor(ROWS), 20);
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
    // The deferred edits below go on from this editor: a fresh one could
    // write the value A1 already holds, and that edit runs nothing.
    let mut a1 = Editor::new();
    let (incr, reference) = {
        let mut reference_model = with_mode(&cycle, false);
        let mut reference_editor = Editor::new();
        let (incr, reference) = interleaved(
            WARM_UP,
            incr_runs(),
            REFERENCE_RUNS,
            edit_step(&mut model, &mut a1, 1),
            edit_step(&mut reference_model, &mut reference_editor, 1),
        );
        (median(incr), median(reference))
    };
    let runs = incr_probe::counts(&model)
        .expect("incremental")
        .reference_runs
        - before;
    assert_eq!(
        runs as usize,
        WARM_UP + incr_runs(),
        "every edit of A1 runs the reference"
    );
    gate.ratio(
        "reference recalc (an edit read by a cycle pin)",
        incr,
        reference,
        1.3,
    );
    // A reference recalc leaves the cone it reached to the next recalc that
    // needs none: this is what that recalc pays, beside the A1 row's cost.
    let mut after = Editor::new();
    let before = incr_probe::counts(&model)
        .expect("incremental")
        .reference_runs;
    let deferred = median(
        (0..REFERENCE_RUNS)
            .map(|_| {
                a1.edit(&mut model, 1);
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

    gate.exit_code()
}

#[cfg(target_os = "linux")]
mod instructions {
    use std::io::Error;

    /// `perf_event_attr` at `PERF_ATTR_SIZE_VER0`, the fields this uses.
    #[repr(C)]
    #[derive(Default)]
    struct Attr {
        kind: u32,
        size: u32,
        config: u64,
        sample_period: u64,
        sample_type: u64,
        read_format: u64,
        flags: u64,
        wakeup_events: u32,
        bp_type: u32,
        config1: u64,
    }

    const PERF_TYPE_HARDWARE: u32 = 0;
    const PERF_COUNT_HW_INSTRUCTIONS: u64 = 1;
    const DISABLED: u64 = 1 << 0;
    const EXCLUDE_KERNEL: u64 = 1 << 5;
    const EXCLUDE_HV: u64 = 1 << 6;
    const IOC_ENABLE: libc::c_ulong = 0x2400;
    const IOC_DISABLE: libc::c_ulong = 0x2401;
    const IOC_RESET: libc::c_ulong = 0x2403;

    /// This thread's user-space instruction counter.
    pub struct Counter(libc::c_int);

    impl Counter {
        pub fn open() -> Result<Self, String> {
            let attr = Attr {
                kind: PERF_TYPE_HARDWARE,
                size: std::mem::size_of::<Attr>() as u32,
                config: PERF_COUNT_HW_INSTRUCTIONS,
                flags: DISABLED | EXCLUDE_KERNEL | EXCLUDE_HV,
                ..Attr::default()
            };
            // SAFETY: `attr` is a valid VER0 attribute that outlives the call.
            let fd = unsafe {
                libc::syscall(
                    libc::SYS_perf_event_open,
                    &attr as *const Attr,
                    0,
                    -1,
                    -1,
                    0,
                )
            };
            if fd < 0 {
                return Err(format!("perf_event_open: {}", Error::last_os_error()));
            }
            Ok(Counter(fd as libc::c_int))
        }

        pub fn start(&self) {
            // SAFETY: the fd is this counter's open perf event.
            unsafe {
                libc::ioctl(self.0, IOC_RESET, 0);
                libc::ioctl(self.0, IOC_ENABLE, 0);
            }
        }

        pub fn stop(&self) -> Result<u64, String> {
            let mut count = 0_u64;
            // SAFETY: the fd is this counter's open perf event, and a read
            // of it with no read_format fills one u64.
            let read = unsafe {
                libc::ioctl(self.0, IOC_DISABLE, 0);
                libc::read(self.0, (&mut count as *mut u64).cast(), 8)
            };
            if read != 8 {
                return Err(format!("read: {}", Error::last_os_error()));
            }
            Ok(count)
        }
    }

    impl Drop for Counter {
        fn drop(&mut self) {
            // SAFETY: the fd is owned by this counter.
            unsafe { libc::close(self.0) };
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod instructions {
    pub struct Counter;

    impl Counter {
        pub fn open() -> Result<Self, String> {
            Err("no perf counter on this platform".to_string())
        }

        pub fn start(&self) {}

        pub fn stop(&self) -> Result<u64, String> {
            Err("no perf counter on this platform".to_string())
        }
    }
}
