#![feature(allocator_api)]
#![feature(slice_ptr_get)]
#![feature(thread_id_value)]
#![allow(dead_code)]

extern crate alloc;

mod buddy_alloc;
mod cpuid;
mod tree;

#[cfg(test)]
mod test {
    use super::*;
    use buddy_alloc::BuddyAlloc;
    use std::{
        thread,
        alloc::Global,
    };

    const PAGE_SIZE: usize = 1 << 12;

    struct Cpu;

    impl cpuid::Cpu for Cpu {
        fn current_cpu() -> usize {
           thread::current().id().as_u64().get() as usize
        }
    }

    #[test]
    fn basic_create() {
        let _buddy = BuddyAlloc::<PAGE_SIZE, Cpu, _>::new(0, 10, &Global).unwrap();
        let _buddy = BuddyAlloc::<PAGE_SIZE, Cpu, _>::new(1000, 1000, &Global).unwrap();
    }

    #[test]
    fn basic_alloc() {
        let buddy = BuddyAlloc::<PAGE_SIZE, Cpu, _>::new(0, 16, &Global).unwrap();

        for _ in 0..8 {
            assert!(buddy.alloc(2).is_some())
        }

        assert!(buddy.alloc(1).is_none());
    }

    #[test]
    fn basic_free() {
        let buddy = BuddyAlloc::<PAGE_SIZE, Cpu, _>::new(0, 16, &Global).unwrap();
        let mut addrs = Vec::with_capacity(16);

        for _ in 0..5 {
            addrs.push(buddy.alloc(2).unwrap());
        }

        for i in addrs {
            buddy.free(i, 2);
        }

        for _ in 0..8 {
            assert!(buddy.alloc(2).is_some())
        }
    }

}
