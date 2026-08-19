//! Real-time safety harness (spec section 3.3).
//!
//! `assert_realtime` marks a scope as "the audio callback". Every DSP test
//! wraps its call to `process()` in it. When this crate is compiled for
//! tests with the `rt-assert` feature, the test binary's global allocator is
//! swapped for one that panics on any allocation made while the scope is
//! active, catching the most common way an audio thread glitches.

use std::cell::Cell;

thread_local! {
    static IN_AUDIO_CALLBACK: Cell<bool> = const { Cell::new(false) };
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
    }
}

/// Runs `f` inside a [`RealtimeGuard`].
pub fn assert_realtime<T>(f: impl FnOnce() -> T) -> T {
    let _guard = RealtimeGuard::enter();
    f()
}

#[cfg(all(test, feature = "rt-assert"))]
mod panicking_allocator {
    use super::IN_AUDIO_CALLBACK;
    use std::alloc::{GlobalAlloc, Layout, System};

    struct RtAssertAlloc;

    unsafe impl GlobalAlloc for RtAssertAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            trap();
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            trap();
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            trap();
            unsafe { System.realloc(ptr, layout, new_size) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            trap();
            unsafe { System.alloc_zeroed(layout) }
        }
    }

    // Clear the flag before panicking: the panic path (message formatting,
    // unwind tables) allocates too, and would otherwise recurse into this
    // same trap and blow the stack instead of failing the test cleanly.
    fn trap() {
        let tripped = IN_AUDIO_CALLBACK.with(|flag| flag.get());
        if tripped {
            IN_AUDIO_CALLBACK.with(|flag| flag.set(false));
            panic!("allocation attempted inside a RealtimeGuard scope");
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
}
