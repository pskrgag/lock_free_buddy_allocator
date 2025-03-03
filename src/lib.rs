//! Scalable lock-free buddy system allocator implementation
//!
//! Algorithm is based on [Andrea Scarselli's thesis](https://alessandropellegrini.it/publications/tScar17.pdf).
//!
//! # Into
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
//! # Node state
//!
//! Node state contains information about the node itself and it's subtree. Node can be in 4
//! different states:
//!     - Occupied -- whole node is allocated
//!     - Partially occupied -- left or right sub-tree have occupied nodes
//!     - Coalescing -- is going to be freed soon
//!     - Free -- node is free
//!
//! which sums to 5 bits of space.
//!
//! State does not contain information about the parent, which makes allocation faster, since it's not
//! required to walk sub-tree to update each children state.
//!
//! To reduce number of CAS instructions, node state contains information about 15 connected nodes
//! (4 levels of the tree). Since it's not possible to compact 15 * 5 bits into atomic word
//! (without considering double CMPXCH), only leaf nodes contain all 5 bits, but other 8 nodes
//! contain just free / occupied bits.
//!
//! # Example usage
//!
//! ```rust
//! #![feature(allocator_api)]
//! #![feature(thread_id_value)]
//!
//! extern crate lock_free_buddy_allocator;
//!
//! use lock_free_buddy_allocator::buddy_alloc::BuddyAlloc;
//! use lock_free_buddy_allocator::cpuid;
//!
//! use std::{alloc::Global, thread};
//!
//! struct Cpu;
//!
//! impl cpuid::Cpu for Cpu {
//!     fn current_cpu() -> usize {
//!         thread::current().id().as_u64().get() as usize
//!     }
//! }
//!
//! fn main() {
//!     let buddy: BuddyAlloc<Cpu, std::alloc::Global> =
//!         BuddyAlloc::<Cpu, _>::new(0, 10, &Global).unwrap();
//!
//!     buddy.free(buddy.alloc(2).unwrap(), 2);
//! }
//! ```

#![no_std]
#![feature(allocator_api)]
#![feature(slice_ptr_get)]
#![cfg_attr(test, feature(thread_id_value))]
#![cfg_attr(test, feature(rustc_private))]
#![cfg_attr(test, feature(non_null_from_ref))]
#![allow(unexpected_cfgs)]

#[cfg(loom)]
pub(crate) use loom::sync::atomic::{AtomicUsize, Ordering};

#[cfg(not(loom))]
pub(crate) use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
#[macro_use]
extern crate std;

pub mod buddy_alloc;
pub mod cpuid;
mod state;
mod tree;

#[cfg(test)]
#[cfg(not(miri))]
#[cfg(not(loom))]
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
                if other.start + PAGE_SIZE * (1 << other.size) > self.start {
                    true
                } else {
                    false
                }
            } else if self.start + (1 << self.size) * PAGE_SIZE > other.start {
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

            vec.push(MemRegion::new(1, 2));
            vec.push(MemRegion::new(2, 1));

            assert!(intersection(vec));
        }

        {
            let mut vec = Vec::with_capacity(2);

            vec.push(MemRegion::new(0, 0));
            vec.push(MemRegion::new(PAGE_SIZE, 0));
            vec.push(MemRegion::new(PAGE_SIZE * 2, 0));

            assert!(!intersection(vec));
        }

        {
            let mut vec = Vec::with_capacity(2);

            vec.push(MemRegion::new(0, 0));
            vec.push(MemRegion::new(PAGE_SIZE, 2));
            vec.push(MemRegion::new(PAGE_SIZE * 2, 1));

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
            vec.push(MemRegion::new(buddy.alloc(1).unwrap(), 1));
        }

        assert!(!intersection(vec));
        assert!(buddy.alloc(1).is_none());
    }

    #[test]
    fn basic_alloc_1() {
        let buddy = BuddyAlloc::<Cpu, _>::new(0, 10, &Global).unwrap();
        let mut vec = Vec::with_capacity(8);

        for _ in 0..512 {
            vec.push(MemRegion::new(buddy.alloc(1).unwrap(), 1));
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
                    res.lock()
                        .unwrap()
                        .push(MemRegion::new(buddy.alloc(2).unwrap(), 2));
                }
            }
        });

        for _ in 0..(1024 >> 2) / 2 {
            res_vec
                .lock()
                .unwrap()
                .push(MemRegion::new(buddy.alloc(2).unwrap(), 2));
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
                for _ in 0..((1 << 10) / 2) >> 4 {
                    if let Some(a) = buddy.alloc(4) {
                        res.lock().unwrap().push(MemRegion::new(a, 4));
                    }
                }
            }
        });

        for _ in 0..(1024 / 2) >> 2 {
            if let Some(a) = buddy.alloc(2) {
                res_vec.lock().unwrap().push(MemRegion::new(a, 2));
            }
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

    #[test]
    fn bug_8threads() {
        for _ in 0..5 {
            let buddy = BuddyAlloc::<Cpu, _>::new(0, 13, &Global).unwrap();
            let b = Arc::new(buddy);

            std::thread::scope(|s| {
                let w_ths: Vec<_> = (0..8)
                    .map(|_| {
                        let b = b.clone();
                        s.spawn(move || {
                            'outter: for _ in 0..((1 << 13) / 8) {
                                for _ in 0..10 {
                                    if b.alloc(0).is_some() {
                                        continue 'outter;
                                    }
                                }

                                panic!("allocation failure");
                            }
                        })
                    })
                    .collect();

                for th in w_ths {
                    th.join().unwrap();
                }
            });
            assert!(b.clone().alloc(0).is_none());
        }
    }

    #[test]
    fn parallel_alloc_free() {
        let buddy = Arc::new(BuddyAlloc::<Cpu, _>::new(0, 12, &Global).unwrap());
        let allocated = (0..1 << 11)
            .map(|_| buddy.alloc(0).unwrap())
            .collect::<Vec<_>>();
        let buddy_free = buddy.clone();

        let free_t = thread::spawn(move || {
            for i in allocated {
                buddy_free.free(i, 0);
            }
        });

        let alloc_t = thread::spawn(move || {
            let _ = (0..1 << 11)
                .map(|_| buddy.alloc(0).unwrap())
                .collect::<Vec<_>>();
        });

        free_t.join().unwrap();
        alloc_t.join().unwrap();
    }
}

#[cfg(test)]
#[cfg(miri)]
mod test_miri {
    use super::*;
    use buddy_alloc::BuddyAlloc;
    use std::{alloc::Global, thread};

    struct Cpu;

    impl cpuid::Cpu for Cpu {
        fn current_cpu() -> usize {
            thread::current().id().as_u64().get() as usize
        }
    }

    #[test]
    fn alloc_free() {
        let buddy = BuddyAlloc::<Cpu, _>::new(0, 4, &Global).unwrap();

        for _ in 0..8 {
            buddy.free(buddy.alloc(1).unwrap(), 1);
        }
    }
}

#[cfg(test)]
#[cfg(loom)]
mod test_loom {
    use super::*;
    use buddy_alloc::BuddyAlloc;
    use loom::thread;
    use std::alloc::AllocError;
    use std::alloc::{Allocator, Layout};
    use std::ptr::NonNull;
    use std::sync::Arc;

    extern crate libc;

    struct Cpu;
    struct Alloc;

    impl cpuid::Cpu for Cpu {
        fn current_cpu() -> usize {
            0
        }
    }

    unsafe impl Allocator for Alloc {
        unsafe fn deallocate(&self, ptr: NonNull<u8>, _layout: Layout) {
            libc::free(ptr.as_ptr() as *mut libc::c_void);
        }

        fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
            let p = unsafe { libc::malloc(layout.size()) };

            if p.is_null() {
                Err(AllocError)
            } else {
                unsafe {
                    Ok(NonNull::from_ref(core::slice::from_raw_parts_mut(
                        p as *mut u8,
                        layout.size(),
                    )))
                }
            }
        }
    }

    #[test]
    fn alloc_alloc() {
        loom::model(move || {
            let buddy = Arc::new(BuddyAlloc::<Cpu, _>::new(0, 1, &Alloc).unwrap());
            let thread = thread::spawn({
                let buddy = buddy.clone();

                move || {
                    buddy.alloc(0).unwrap();
                }
            });

            buddy.alloc(0).unwrap();
            thread.join().unwrap();
        });
    }

    #[test]
    fn alloc_free() {
        loom::model(move || {
            let buddy = Arc::new(BuddyAlloc::<Cpu, _>::new(0, 1, &Alloc).unwrap());
            let thread = thread::spawn({
                let buddy = buddy.clone();

                move || {
                    buddy.alloc(0).unwrap();
                }
            });

            buddy.free(buddy.alloc(0).unwrap(), 0);
            thread.join().unwrap();
        });
    }
}
