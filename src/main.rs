#![feature(allocator_api)]
#![feature(slice_ptr_get)]
#![feature(thread_id_value)]

#[macro_use]
extern crate static_assertions;
extern crate alloc;

mod tree;

use crate::tree::tree::Tree;
use crate::tree::cpuid::Cpu;

struct A;

impl Cpu for A {
    fn current_cpu() -> usize {
        std::thread::current().id().as_u64().get() as usize
    }

    fn cpu_count() -> usize {
        1
    }
}

fn main() {
   let tree = Tree::<4096, 1, A>::new(0, 16, &alloc::alloc::Global).expect("Failed to crate");
   println!("start {}", tree.alloc(2).expect("Failed to allocate memory"));
   println!("start {}", tree.alloc(2).expect("Failed to allocate memory"));
   println!("start {}", tree.alloc(2).expect("Failed to allocate memory"));
   println!("start {}", tree.alloc(2).expect("Failed to allocate memory"));
   println!("start {}", tree.alloc(2).expect("Failed to allocate memory"));
   println!("start {}", tree.alloc(2).expect("Failed to allocate memory"));
   println!("start {}", tree.alloc(2).expect("Failed to allocate memory"));
   println!("start {}", tree.alloc(2).expect("Failed to allocate memory"));
}
