//! Scalable lock-free buddy system allocator implementation
//!
//! Algorithm is based on [Andrea Scarselli's thesis](https://alessandropellegrini.it/publications/tScar17.pdf).
//!
//! Algorithm works on top of pre-allocated full binary tree where each node represents a
//! contiguous memory region with size equal to some power of two. For non-leaf nodes, right and
//! left children represent nodes that occupy the same range, but with two times smaller size. For
//! example tree for range 0..2 looks like following
//!
//! ```md
//!           ----------
//!          | start: 0 |
//!      +---| order: 1 | ------+
//!      |    ---------         |
//!      v                      v
//!   ----------            ----------
//!  | start: 0 |          | start: 1 |
//!  | order: 0 |          | order: 0 |
//!   ---------             ---------
//!
//! ```
//!
//! Each node also has a state variable that contain various info about itself and part of
//! its sub-tree. To reduce contention and number of expensive atomic operations, state of
//! 15 connected nodes is packed into one atomic word.
//!
//!
//!

#![no_std]
#![feature(allocator_api)]
#![feature(slice_ptr_get)]
#![cfg_attr(test, feature(thread_id_value))]

#[cfg(test)]
#[macro_use]
extern crate std;

pub mod buddy_alloc;
pub mod cpuid;
mod tree;
mod state;

#[cfg(test)]
mod test {
    use super::*;
    use buddy_alloc::BuddyAlloc;
    use std::{
        alloc::Global,
        sync::{Arc, Mutex},
        thread,
        vec::Vec,
    };

    const PAGE_SIZE: usize = 1 << 12;

    struct Cpu;

    impl cpuid::Cpu for Cpu {
        fn current_cpu() -> usize {
            thread::current().id().as_u64().get() as usize
        }
    }

    #[derive(Eq, Clone, Copy, Hash, Debug)]
    pub struct MemRegion {
        pub start: usize,
        pub size: usize,
    }

    impl MemRegion {
        pub fn new(start: usize, size: usize) -> Self {
            Self { start, size }
        }
    }

    impl PartialEq for MemRegion {
        fn eq(&self, other: &Self) -> bool {
            if self.start > other.start {
                if other.start + other.size > self.start {
                    true
                } else {
                    false
                }
            } else if self.start + self.size > other.start {
                true
            } else {
                false
            }
        }
    }

    pub fn intersection(nums: Vec<MemRegion>) -> bool {
        for i in 0..nums.len() {
            let mut new = nums.clone();

            new.remove(i);

            for j in new {
                if j == nums[i] {
                    return true;
                }
            }
        }

        false
    }

    #[test]
    fn test_helpers() {
        {
            let mut vec = Vec::with_capacity(2);

            vec.push(MemRegion::new(1, 10));
            vec.push(MemRegion::new(2, 5));

            assert!(intersection(vec));
        }

        {
            let mut vec = Vec::with_capacity(2);

            vec.push(MemRegion::new(1, 10));
            vec.push(MemRegion::new(11, 10));
            vec.push(MemRegion::new(21, 10));

            assert!(!intersection(vec));
        }

        {
            let mut vec = Vec::with_capacity(2);

            vec.push(MemRegion::new(1, 10));
            vec.push(MemRegion::new(10, 10));
            vec.push(MemRegion::new(21, 10));

            assert!(intersection(vec));
        }
    }

    #[test]
    fn basic_create() {
        assert!(BuddyAlloc::<Cpu, _>::new(0, 10, &Global).is_some());
        assert!(BuddyAlloc::<Cpu, _>::new(0, 4, &Global).is_some());
    }

    #[test]
    fn alloc_child() {
        let buddy = BuddyAlloc::<Cpu, _>::new(0, 10, &Global).unwrap();

        assert!(buddy.__try_alloc_node(513).is_none());
        assert!(buddy.__try_alloc_node(513 * 2 + 1).is_some());
    }

    #[test]
    fn basic_alloc() {
        let buddy = BuddyAlloc::<Cpu, _>::new(0, 4, &Global).unwrap();
        let mut vec = Vec::with_capacity(8);

        for _ in 0..8 {
            vec.push(MemRegion::new(buddy.alloc(1).unwrap(), 2 * PAGE_SIZE));
        }

        assert!(!intersection(vec));
        assert!(buddy.alloc(1).is_none());
    }

    #[test]
    fn basic_alloc_1() {
        let buddy = BuddyAlloc::<Cpu, _>::new(0, 10, &Global).unwrap();
        let mut vec = Vec::with_capacity(8);

        for _ in 0..512 {
            vec.push(MemRegion::new(buddy.alloc(1).unwrap(), 2 * PAGE_SIZE));
        }

        assert!(!intersection(vec));
        assert!(buddy.alloc(1).is_none());
    }

    #[test]
    fn basic_free() {
        let buddy = BuddyAlloc::<Cpu, _>::new(0, 4, &Global).unwrap();
        let mut addrs = Vec::with_capacity(16);

        for _ in 0..4 {
            addrs.push(buddy.alloc(2).unwrap());
        }

        for i in addrs {
            buddy.free(i, 2);
        }

        for _ in 0..4 {
            assert!(buddy.alloc(2).is_some())
        }
    }

    #[test]
    fn multi_threaded_alloc_same_size() {
        let buddy = Arc::new(BuddyAlloc::<Cpu, _>::new(0, 10, &Global).unwrap());
        let res_vec = Arc::new(Mutex::new(Vec::<MemRegion>::new()));

        let thread = thread::spawn({
            let buddy = buddy.clone();
            let res = res_vec.clone();
            move || {
                for _ in 0..(1024 >> 2) / 2 {
                    res.lock().unwrap().push(MemRegion::new(
                        buddy.alloc(2).unwrap(),
                        (1 << 2) * PAGE_SIZE,
                    ));
                }
            }
        });

        for _ in 0..(1024 >> 2) / 2 {
            res_vec.lock().unwrap().push(MemRegion::new(
                buddy.alloc(2).unwrap(),
                (1 << 2) * PAGE_SIZE,
            ));
        }

        thread.join().unwrap();

        assert!(buddy.alloc(1).is_none());
        assert!(!intersection(
            Arc::try_unwrap(res_vec).unwrap().into_inner().unwrap()
        ));
    }

    #[test]
    fn multi_threaded_alloc_diff_size() {
        let buddy = Arc::new(BuddyAlloc::<Cpu, _>::new(0, 10, &Global).unwrap());
        let res_vec = Arc::new(Mutex::new(Vec::<MemRegion>::new()));

        let thread = thread::spawn({
            let buddy = buddy.clone();
            let res = res_vec.clone();
            move || {
                for _ in 0..(1024 / 2) >> 4 {
                    res.lock()
                        .unwrap()
                        .push(MemRegion::new(buddy.alloc(4).unwrap(), 4));
                }
            }
        });

        for _ in 0..(1024 / 2) >> 2 {
            res_vec
                .lock()
                .unwrap()
                .push(MemRegion::new(buddy.alloc(2).unwrap(), 2));
        }

        thread.join().unwrap();

        for i in &*res_vec.lock().unwrap() {
            buddy.free(i.start, i.size);
        }

        assert!(buddy.alloc(10).is_some());

        assert!(!intersection(
            Arc::try_unwrap(res_vec).unwrap().into_inner().unwrap()
        ));
    }

    #[test]
    fn buddy_alloc_test() {
        let buddy = Arc::new(BuddyAlloc::<Cpu, _>::new(0, 12, &Global).unwrap());

        let w_ths: Vec<_> = (0..10)
            .map(|_| {
                let buddy = buddy.clone();
                thread::spawn(move || {
                    for _ in 0..(4096 >> 3) / 10 {
                        buddy.alloc(3).unwrap();
                    }
                })
            })
            .collect();

        for th in w_ths {
            th.join().unwrap();
        }
    }
}
