//! Definition of a trait for current CPU id
//!

/// Trait for current thread id.
pub trait Cpu {
    /// Returns ID of a current running CPU.
    fn current_cpu() -> usize;
}
