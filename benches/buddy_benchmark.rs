#![feature(allocator_api)]
#![cfg_attr(test, feature(thread_id_value))]
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, PlotConfiguration};

extern crate lock_free_buddy_allocator;

use lock_free_buddy_allocator::buddy_alloc::BuddyAlloc;
use lock_free_buddy_allocator::cpuid;

use std::{
    alloc::{Allocator, Global},
    sync::Arc,
    thread,
};

struct Cpu;

const TEST_ORDER: u8 = 13;

impl cpuid::Cpu for Cpu {
    fn current_cpu() -> usize {
        thread::current().id().as_u64().get() as usize
    }
}

fn buddy_alloc_test<A: Allocator>(n: usize, buddy: BuddyAlloc<Cpu, A>) {
    let b = Arc::new(buddy);

    std::thread::scope(|s| {
        let w_ths: Vec<_> = (0..n)
            .map(|_| {
                let b = b.clone();
                s.spawn(move || {
                    for _ in 0..((1 << TEST_ORDER) / n) {
                        b.alloc(0).unwrap();
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
    let plot_config = PlotConfiguration::default();
    let mut group = c.benchmark_group("Single page alloc");

    group.plot_config(plot_config);

    for s in &[1, 2, 4, 8, 16] {
        group.bench_with_input(BenchmarkId::new("Single page alloc", s), s, |b, i| {
            b.iter(|| {
                buddy_alloc_test(
                    *i,
                    BuddyAlloc::<Cpu, _>::new(0, TEST_ORDER, &Global).unwrap(),
                )
            });
        });
    }

    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
