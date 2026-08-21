//! Real-time safety harness (spec section 3.3).
//!
//! `assert_realtime` marks a scope as "the audio callback". Every DSP test
//! wraps its call to `process()` in it. When this crate is compiled for
//! tests with the `rt-assert` feature, the test binary's global allocator is
//! swapped for one that fails any test that allocates while the scope is
//! active, catching the most common way an audio thread glitches.

use std::cell::Cell;

thread_local! {
    static IN_AUDIO_CALLBACK: Cell<bool> = const { Cell::new(false) };
    static VIOLATION: Cell<bool> = const { Cell::new(false) };
}

/// Marks the current thread as executing the audio callback until dropped.
pub struct RealtimeGuard(());

impl RealtimeGuard {
    pub fn enter() -> Self {
        IN_AUDIO_CALLBACK.with(|flag| flag.set(true));
        RealtimeGuard(())
    }
}

impl Drop for RealtimeGuard {
    fn drop(&mut self) {
        IN_AUDIO_CALLBACK.with(|flag| flag.set(false));
        let violated = VIOLATION.with(|v| v.replace(false));
        // Rust does not reliably support unwinding a panic out of a
        // #[global_allocator] method under optimisation: a panic thrown
        // from inside `alloc()` escapes `catch_unwind` in a release build
        // (confirmed independently of this crate — a minimal
        // global-allocator-panics-inside-alloc repro exits 101, uncaught,
        // under `rustc -O`, while the identical debug build is caught
        // cleanly), so the allocator below only records that a violation
        // happened and lets the real allocation proceed; the actual panic
        // fires here instead, in ordinary post-scope code where unwinding
        // is reliable in every profile. Guarded by `panicking()` so a
        // violation recorded during a callback that was *already*
        // panicking for its own reason doesn't turn one panic into a
        // double panic (which aborts the process instead of failing the
        // test).
        if violated && !std::thread::panicking() {
            panic!("allocation attempted inside a RealtimeGuard scope");
        }
    }
}

/// Runs `f` inside a [`RealtimeGuard`].
pub fn assert_realtime<T>(f: impl FnOnce() -> T) -> T {
    let _guard = RealtimeGuard::enter();
    f()
}

#[cfg(all(test, feature = "rt-assert"))]
mod panicking_allocator {
    use super::{IN_AUDIO_CALLBACK, VIOLATION};
    use std::alloc::{GlobalAlloc, Layout, System};

    struct RtAssertAlloc;

    unsafe impl GlobalAlloc for RtAssertAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record_if_tripped();
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            record_if_tripped();
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record_if_tripped();
            unsafe { System.realloc(ptr, layout, new_size) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record_if_tripped();
            unsafe { System.alloc_zeroed(layout) }
        }
    }

    fn record_if_tripped() {
        if IN_AUDIO_CALLBACK.with(|flag| flag.get()) {
            VIOLATION.with(|v| v.set(true));
        }
    }

    #[global_allocator]
    static ALLOC: RtAssertAlloc = RtAssertAlloc;
}

#[cfg(test)]
mod tests {
    use super::assert_realtime;

    #[test]
    fn realtime_guard_permits_non_allocating_work() {
        assert_eq!(assert_realtime(|| 2_i32 + 2), 4);
    }

    #[test]
    #[cfg(feature = "rt-assert")]
    fn realtime_guard_traps_allocation() {
        let outcome = std::panic::catch_unwind(|| assert_realtime(|| vec![1u8]));
        assert!(outcome.is_err(), "allocating inside the guard should panic");
    }

    #[test]
    fn guard_releases_the_flag_on_drop() {
        assert_realtime(|| {});
        // A second, unguarded allocation must not be affected by the first
        // guard having run and dropped.
        let v: Vec<u8> = Vec::with_capacity(1);
        assert!(v.is_empty());
    }

    #[test]
    #[cfg(feature = "rt-assert")]
    fn a_clean_scope_after_a_trapped_one_does_not_spuriously_fail() {
        let _ = std::panic::catch_unwind(|| assert_realtime(|| vec![1u8]));
        // The violation flag must not leak into the next scope.
        assert_eq!(assert_realtime(|| 2_i32 + 2), 4);
    }

    #[test]
    #[cfg(feature = "rt-assert")]
    fn the_callbacks_own_panic_is_not_swallowed_by_a_concurrent_violation() {
        let outcome = std::panic::catch_unwind(|| {
            assert_realtime(|| {
                // Must be a real heap allocation, not a stack array, to
                // trip the violation flag: that's the scenario under test.
                #[allow(clippy::useless_vec)]
                let _v = vec![1u8];
                panic!("boom");
            })
        });
        let payload = outcome.expect_err("the callback's panic should propagate");
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or_default();
        assert_eq!(message, "boom");
    }
}
