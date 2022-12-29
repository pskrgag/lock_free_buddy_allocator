#![feature(allocator_api)]
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

extern crate lock_free_buddy_allocator;

use lock_free_buddy_allocator::buddy_alloc::BuddyAlloc;
use lock_free_buddy_allocator::cpuid;

use std::{
    alloc::{Allocator, Global},
    num::NonZeroU64,
    sync::Arc,
    thread::{self, ThreadId},
};

const PAGE_SIZE: usize = 1 << 12;

struct Cpu;

impl cpuid::Cpu for Cpu {
    fn current_cpu() -> usize {
        // HACK! Since i don't know how to enable feature under #[cfg(test)] only
        unsafe {
            (*(&thread::current().id() as *const ThreadId as *const u8 as *const NonZeroU64)).get()
                as usize
        }
    }
}

fn buddy_alloc_test<A: Allocator>(n: usize, buddy: BuddyAlloc<PAGE_SIZE, Cpu, A>) {
    let b = Arc::new(buddy);

    std::thread::scope(|s| {
        let w_ths: Vec<_> = (0..n)
            .map(|_| {
                let b = b.clone();
                s.spawn(move || {
                    for _ in 0..512 {
                        b.alloc(8).unwrap();
                    }
                })
            })
            .collect();

        for th in w_ths {
            th.join().unwrap();
        }
    });
}

pub fn criterion_benchmark(c: &mut Criterion) {
    for s in &[1, 5, 10] {
        c.bench_with_input(BenchmarkId::new("lf_buddy_single", s), s, |b, i| {
            b.iter(|| {
                buddy_alloc_test(
                    *i,
                    BuddyAlloc::<PAGE_SIZE, Cpu, _>::new(0, *s * 4096, &Global).unwrap(),
                )
            });
        });
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
