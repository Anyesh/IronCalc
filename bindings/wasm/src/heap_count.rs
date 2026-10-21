//! A counting allocator for the incr benches on Node: the bytes the heap
//! holds live, which `memory.buffer` can't tell because a wasm memory never
//! shrinks and freed blocks are reused.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use wasm_bindgen::prelude::wasm_bindgen;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        LIVE.fetch_add(new_size, Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[doc(hidden)]
#[wasm_bindgen(js_name = "liveHeapBytes", skip_typescript)]
pub fn live_heap_bytes() -> f64 {
    LIVE.load(Ordering::Relaxed) as f64
}
