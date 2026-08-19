//! Pure DSP and mixing engine. Milestones after M0 add the strips, buses
//! and filters described in `docs/SPEC.md`; this crate currently holds only
//! the real-time safety harness those milestones are required to test with.

// Unsafe is needed to implement `GlobalAlloc` for the test-only allocator in
// `rt_assert`, so the forbid only applies to the shipped (non-test) build.
#![cfg_attr(not(test), forbid(unsafe_code))]

pub mod rt_assert;
