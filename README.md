# Scalable lock-free buddy system allocator

Algorithm source: https://hpdcs.github.io/ths/scar17.pdf

## Brief

The buddy memory allocation technique is a memory allocation algorithm that divides memory into partitions to try to satisfy a memory request as suitably as possible. This system makes use of splitting memory into halves to try to give a best fit. According to Donald Knuth, the buddy system was invented in 1963 by Harry Markowitz, and was first described by Kenneth C. Knowlton (published 1965) The Buddy memory allocation is relatively easy to implement. It supports limited but efficient splitting and coalescing of memory blocks.


This allocator intended for OS purposes, but might be also used in user-space.

Allocator requiers any backend allocator for allocating internal data structures. In case of OS it might be
allocator based on static memory; in case of user-space std::alloc::Global is good candidate. Relying on Global allocator
seems to be wrong, since buddy system allocator is widly used as page allocator and Global may be not initialized at 
the point of buddy initialization

## Example

```rust
#![feature(allocator_api)]
#![feature(thread_id_value)]

extern crate lock_free_buddy_allocator;

use lock_free_buddy_allocator::buddy_alloc::BuddyAlloc;
use lock_free_buddy_allocator::cpuid;

use std::{alloc::Global, thread};

const PAGE_SIZE: usize = 1 << 12;

struct Cpu;

impl cpuid::Cpu for Cpu {
    fn current_cpu() -> usize {
        thread::current().id().as_u64().get() as usize
    }
}

fn main() {
    let buddy: BuddyAlloc<PAGE_SIZE, Cpu, std::alloc::Global> =
        BuddyAlloc::<PAGE_SIZE, Cpu, _>::new(0, 4096, &Global).unwrap();

    buddy.free(buddy.alloc(2).unwrap(), 2);
}

```
## License

`lock_free_buddy_allocator` is distributed under the MIT License, (See `LICENSE`).

