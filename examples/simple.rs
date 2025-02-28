#![feature(allocator_api)]
#![feature(thread_id_value)]

extern crate lock_free_buddy_allocator;

use lock_free_buddy_allocator::buddy_alloc::BuddyAlloc;
use lock_free_buddy_allocator::cpuid;

use std::{alloc::Global, thread};

struct Cpu;

impl cpuid::Cpu for Cpu {
    fn current_cpu() -> usize {
        thread::current().id().as_u64().get() as usize
    }
}

fn main() {
    let buddy: BuddyAlloc<Cpu, std::alloc::Global> =
        BuddyAlloc::<Cpu, _>::new(0, 4096, &Global).unwrap();

    buddy.free(buddy.alloc(2).unwrap(), 2);
}
