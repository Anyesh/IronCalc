//! A cell at the last row in many columns must not make the incremental
//! evaluator allocate per-column tables sized to that row.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use ironcalc_base::cell::CellValue;
use ironcalc_base::Model;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grow(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= layout.size() {
            grow(new_size - layout.size());
        } else {
            LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const LAST_ROW: i32 = 1_048_576;
const COLUMNS: i32 = 2000;
const MIB: usize = 1 << 20;

#[test]
fn last_row_in_many_columns_stays_small() {
    let mut model = Model::new_empty("sparse", "en", "UTC", "en").expect("an empty model");
    for column in 1..=COLUMNS {
        model
            .set_user_input(0, LAST_ROW, column, "=1".to_string())
            .expect("a valid input");
    }
    model.set_incremental(true);

    let live_before = LIVE.load(Ordering::Relaxed);
    PEAK.store(live_before, Ordering::Relaxed);
    model.evaluate();
    let added = PEAK.load(Ordering::Relaxed) - live_before;
    let added_mib = added as f64 / MIB as f64;

    println!("first incremental evaluate peak: {added_mib:.1} MiB");
    assert!(
        added < 64 * MIB,
        "the first incremental evaluate peaked at {added_mib:.1} MiB above the model"
    );
    for column in [1, COLUMNS / 2, COLUMNS] {
        assert_eq!(
            model.get_cell_value_by_index(0, LAST_ROW, column),
            Ok(CellValue::Number(1.0))
        );
    }
}
