//! `EVAL_CTX`: how a compute reaches the model. A query's closure captures
//! only its position, because the runtime requires `Send + Sync + 'static`
//! closures and the model is none of these. The caller of every `rt.get`
//! arms the context from the `&mut Model` it holds and does not touch that
//! reference until the get returns, so the compute's `&mut` is the only live
//! one.

use std::cell::Cell;
use std::ptr;

use crate::model::Model;

thread_local! {
    static EVAL_CTX: Cell<*mut Model<'static>> = const { Cell::new(ptr::null_mut()) };
    /// How many `SheetReadAllowed` the running compute holds.
    static SHEET_READS: Cell<u32> = const { Cell::new(0) };
}

/// Keeps the context armed; dropping it puts back what was armed before, so
/// the pointer never outlives the borrow it was made from.
pub(crate) struct Armed {
    previous: *mut Model<'static>,
    previous_reads: u32,
}

impl Drop for Armed {
    fn drop(&mut self) {
        EVAL_CTX.with(|ctx| ctx.set(self.previous));
        SHEET_READS.with(|reads| reads.set(self.previous_reads));
    }
}

/// Also starts the compute it is armed for with no read of the sheet
/// allowed: what its caller was allowed to read is not its own.
pub(crate) fn arm(model: &mut Model<'_>) -> Armed {
    let pointer = (model as *mut Model<'_>).cast::<Model<'static>>();
    Armed {
        previous: EVAL_CTX.with(|ctx| ctx.replace(pointer)),
        previous_reads: SHEET_READS.with(|reads| reads.replace(0)),
    }
}

/// While one is held, a compute may read the sheet directly; see
/// `audit_sheet_read`.
pub(crate) struct SheetReadAllowed(());

impl SheetReadAllowed {
    pub(crate) fn new() -> SheetReadAllowed {
        #[cfg(debug_assertions)]
        SHEET_READS.with(|reads| reads.set(reads.get() + 1));
        SheetReadAllowed(())
    }
}

impl Drop for SheetReadAllowed {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        SHEET_READS.with(|reads| reads.set(reads.get() - 1));
    }
}

/// Panics when a compute reads the sheet while it holds no
/// `SheetReadAllowed`: its value would depend on a read the runtime cannot
/// see, and would not be recomputed when what it read changes.
#[cfg(debug_assertions)]
pub(crate) fn audit_sheet_read() {
    assert!(
        !is_armed() || SHEET_READS.with(Cell::get) > 0,
        "a compute of the incremental evaluator read the sheet without a node"
    );
}

pub(crate) fn is_armed() -> bool {
    EVAL_CTX.with(|ctx| !ctx.get().is_null())
}

/// Runs `f` on the armed model. Panics when nothing is armed: a compute ran
/// outside a get made through `arm`.
pub(crate) fn with_model<R>(f: impl FnOnce(&mut Model<'_>) -> R) -> R {
    let pointer = EVAL_CTX.with(|ctx| ctx.get());
    assert!(!pointer.is_null(), "EVAL_CTX is not armed");
    // SAFETY: `arm` took the pointer from a live `&mut Model` whose holder
    // makes no use of it while the guard is up, and `f` cannot keep the
    // reference past the call. The lifetime was erased only to fit the
    // thread-local; `f` sees an unnamed one.
    f(unsafe { &mut *pointer })
}
