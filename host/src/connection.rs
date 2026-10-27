//! Cancellation shared by one native connection and its request worker.
//!
//! The binding is thread-local so nested dialog and harness helpers use the
//! connection lifetime without changing their public, standalone APIs. It is
//! never inherited by child processes or unrelated host connections.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Default)]
pub(crate) struct Cancellation(Arc<AtomicBool>);

thread_local! {
    static CURRENT: RefCell<Option<Cancellation>> = const { RefCell::new(None) };
}

impl Cancellation {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub(crate) fn bind<T>(&self, action: impl FnOnce() -> T) -> T {
        struct Restore(Option<Cancellation>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|current| *current.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(CURRENT.with(|current| current.replace(Some(self.clone()))));
        action()
    }
}

// Closing a writer or rejecting a frame must cancel the worker even when the
// input reader is still blocked on the browser's pipe.
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(crate) struct CancelOnDrop(pub(crate) Cancellation);

pub(crate) fn cancelled() -> bool {
    CURRENT.with(|current| {
        current
            .borrow()
            .as_ref()
            .is_some_and(|cancel| cancel.0.load(Ordering::SeqCst))
    })
}
