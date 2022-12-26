pub trait Cpu {
    fn current_cpu() -> usize;
    fn cpu_count() -> usize;
}
