use core::alloc::Allocator;
use core::sync::atomic::Ordering;

use crate::cpuid::Cpu;
use crate::tree::{Node, Tree};
use core::marker::PhantomData;

const COALESCE_LEFT: usize = 0x8;
const COALESCE_RIGHT: usize = 0x4;

pub struct BuddyAlloc<'a, const PAGE_SIZE: usize, C: Cpu, A: Allocator + 'a> {
    tree: Tree<'a, PAGE_SIZE, A>,
    start: usize,
    size: usize,
    num_pages: usize,
    _d: PhantomData<C>,
}

impl<'a, const PAGE_SIZE: usize, C: Cpu, A: Allocator + 'a>
    BuddyAlloc<'a, PAGE_SIZE, C, A>
{
    #[inline]
    fn level(&self, node: &Node) -> usize {
        self.tree.height() - (node.size / PAGE_SIZE).ilog2() as usize
    }

    #[inline]
    fn is_allocable(val: usize, pos: u8) -> bool {
        if pos < 8 {
            (val & (0x1 << (pos - 1))) == 0
        } else {
            val & ((0x1F << 7) << (5 * (pos - 8))) == 0
        }
    }

    #[inline]
    fn is_occupied(val: usize, pos: u8) -> bool {
        if pos < 8 {
            (val & (0x1 << (pos - 1))) != 0
        } else {
            val & ((0x1 << 6) << (5 * (pos - 7))) != 0
        }
    }

    #[inline]
    fn lock_not_leaf(val: usize, pos: u8) -> usize {
        val | (0x1 << (pos as usize - 1))
    }

    #[inline]
    fn lock_leaf(val: usize, pos: u8) -> usize {
        val | (0x13 << (7 + (5 * (pos as usize - 1 - 7))))
    }

    #[inline]
    fn unlock_not_leaf(val: usize, pos: u8) -> usize {
        val & !(0x1 << (pos as usize - 1))
    }

    #[inline]
    fn unlock_leaf(val: usize, pos: u8) -> usize {
        val & !(0x13 << (7 + (5 * (pos as usize - 1 - 7))))
    }

    #[inline]
    fn clean_left_coalesce(val: usize, pos: u8) -> usize {
        val & !((COALESCE_LEFT << 7) + (5 * (pos as usize - 8)))
    }

    #[inline]
    fn clean_rigth_coalesce(val: usize, pos: u8) -> usize {
        val & !((COALESCE_RIGHT << 7) + (5 * (pos as usize - 8)))
    }

    #[inline]
    fn left_coalesce(val: usize, pos: u8) -> usize {
        val | (COALESCE_LEFT << (7 + (5 * (pos as usize - 8))))
    }

    #[inline]
    fn rigth_coalesce(val: usize, pos: u8) -> usize {
        val | (COALESCE_RIGHT << (7 + (5 * (pos as usize - 8))))
    }

    #[inline]
    fn occupy_left(val: usize, pos: u8) -> usize {
        val | (0x2 << (7 + (5 * (pos as usize - 8))))
    }

    #[inline]
    fn occupy_rigth(val: usize, pos: u8) -> usize {
        val | (0x1 << (7 + (5 * (pos as usize - 8))))
    }

    #[inline]
    fn is_left_coalescing(val: usize, pos: u8) -> bool {
        val == Self::left_coalesce(val, pos)
    }

    #[inline]
    fn is_right_coalescing(val: usize, pos: u8) -> bool {
        val == Self::rigth_coalesce(val, pos)
    }

    #[inline]
    fn clean_left(val: usize, pos: u8) -> usize {
        val & !(0x2 << (7 + (5 * (pos - 8))))
    }

    #[inline]
    fn clean_rigth(val: usize, pos: u8) -> usize {
        val & !(0x1 << (7 + (5 * (pos - 8))))
    }

    #[inline]
    fn is_occupied_rigth(val: usize, pos: u8) -> bool {
        val == Self::occupy_rigth(val, pos)
    }

    #[inline]
    fn is_occupied_left(val: usize, pos: u8) -> bool {
        val == Self::occupy_left(val, pos)
    }

    pub fn new(start: usize, size: usize, backend: &'a A) -> Option<Self> {
        let num_pages = size.next_power_of_two();

        Some(Self {
            tree: Tree::<PAGE_SIZE, A>::new(num_pages, backend)?,
            num_pages: num_pages,
            start: start,
            size: num_pages * PAGE_SIZE,
            _d: PhantomData,
        })
    }

    fn dump(&self) {
        println!("Overall size {}", self.size);
        println!("Num_pages {}", self.num_pages);
        println!("Heigth {}", self.tree.height());
        println!("Num nodes {}", self.tree.node_count());
    }

    pub fn alloc(&self, pages: usize) -> Option<usize> {
        let pages = pages.next_power_of_two();
        let start_node = self.num_pages / pages;
        let last_node = (self.tree.node(start_node).pos * 2 - 1) as usize;
        let mut a = C::current_cpu();
        let mut restared = false;

        if last_node - start_node != 0 {
            a = a % (last_node - start_node);
        } else {
            a = 0;
        }

        a += start_node;

        let started_at = a;

        while {
            match self.try_alloc_node(self.tree.node(a)) {
                None => {
                    println!("Allocated node {}", self.tree.node(a).pos);
                    return Some(self.start + self.tree.node(a).start);
                }
                Some(i) => {
                    if i == 1 {
                        return None;
                    }

                    for i in 2..self.num_pages * 2 - 1 {
                        assert!(self.tree.node(i).container_pos != 0);
                    }

                    a = (i + 1)
                        * (1 << (self.level(self.tree.node(a)) - self.level(self.tree.node(i))));
                }
            }

            if a > last_node {
                a = start_node;
                restared = true;
            }

            !restared || a < started_at
        } {}

        None
    }

    fn check_brother(&self, node: &Node, val: usize) -> bool {
        let parent = self.tree.parent_of(node);
        let l_parent = self.tree.left_of(parent);
        let r_parent = self.tree.left_of(parent);

        if l_parent.pos == node.pos && !Self::is_allocable(val, r_parent.container_pos) {
            true
        } else if r_parent.pos == node.pos && !Self::is_allocable(val, l_parent.container_pos) {
            true
        } else {
            false
        }
    }

    fn unlock_descendants(&self, node: &Node, mut val: usize) -> usize {
        if node.pos as usize * 2 >= self.tree.node_count() {
            return val;
        }

        if !self.tree.is_leaf(self.tree.left_of(node)) {
            val = Self::lock_not_leaf(val, self.tree.left_of(node).container_pos);
            val = Self::lock_not_leaf(val, self.tree.right_of(node).container_pos);

            val = self.unlock_descendants(self.tree.left_of(node), val);
            val = self.unlock_descendants(self.tree.right_of(node), val);
        } else {
            val = Self::lock_leaf(val, self.tree.left_of(node).container_pos);
            val = Self::lock_leaf(val, self.tree.right_of(node).container_pos);
        }

        val
    }

    pub fn unmark(&self, node: &Node, upper_bound: &Node) {
        let mut exit;
        let mut cur;

        'foo: while {
            let parent = self.tree.parent_of(node);
            let mut new_val = node.container.nodes.load(Ordering::Relaxed);
            let old_val = new_val;

            exit = false;

            if self.tree.left_of(parent).pos == node.pos {
                if !Self::is_left_coalescing(new_val, parent.container_pos) {
                    return;
                }

                new_val = Self::clean_left_coalesce(new_val, parent.container_pos);
                new_val = Self::clean_left(new_val, parent.container_pos);

                if Self::is_occupied_rigth(new_val, parent.container_pos) {
                    continue 'foo;
                }
            }

            if self.tree.right_of(parent).pos == node.pos {
                if !Self::is_right_coalescing(new_val, parent.container_pos) {
                    return;
                }

                new_val = Self::clean_rigth_coalesce(new_val, parent.container_pos);
                new_val = Self::clean_rigth(new_val, parent.container_pos);

                if Self::is_occupied_left(new_val, parent.container_pos) {
                    continue 'foo;
                }
            }

            cur = self.tree.parent_of(node);

            'bar: while cur.pos != cur.container.node.pos {
                exit = self.check_brother(cur, new_val);
                if exit {
                    break 'bar;
                }

                new_val = Self::unlock_not_leaf(new_val, self.tree.parent_of(cur).container_pos);
            }

            parent
                .container
                .nodes
                .compare_exchange(old_val, new_val, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        } {}

        if cur.pos != upper_bound.pos && !exit {
            self.unmark(cur, upper_bound)
        }
    }

    pub fn mark(&self, node: &Node, upper_bound: &Node) {
        let parent = self.tree.parent_of(node);

        while {
            let mut new_val = parent.container.nodes.load(Ordering::Relaxed);
            let old_val = new_val;

            if self.tree.left_of(parent).pos == node.pos {
                new_val = Self::left_coalesce(new_val, parent.container_pos);
            } else {
                new_val = Self::rigth_coalesce(new_val, parent.container_pos);
            }

            parent
                .container
                .nodes
                .compare_exchange(old_val, new_val, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        } {}

        if parent.container.node.pos != upper_bound.pos {
            self.mark(parent.container.node, upper_bound);
        }
    }

    pub fn free_node(&self, node: &Node, upper_bound: &Node) {
        let mut exit;

        if node.container.node.pos != upper_bound.pos {
            self.mark(node, upper_bound);
        }

        while {
            let mut new_val = node.container.nodes.load(Ordering::Relaxed);
            let old_val = new_val;
            let mut cur = node;

            exit = false;

            while cur.pos != node.container.node.pos {
                new_val = Self::unlock_not_leaf(new_val, self.tree.parent_of(cur).container_pos);

                exit = self.check_brother(cur, new_val);
                if exit {
                    break;
                }

                cur = self.tree.parent_of(cur);
            }

            if !self.tree.is_leaf(node)
                && node.pos as usize * 2 <= self.tree.node_count()
            {
                new_val = self.unlock_descendants(node, new_val);
            }

            if self.tree.is_leaf(node) {
                new_val = Self::unlock_leaf(new_val, node.container_pos);
            } else {
                new_val = Self::unlock_not_leaf(new_val, node.container_pos);
            }

            node.container
                .nodes
                .compare_exchange(old_val, new_val, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        } {}

        if node.container.node.pos != upper_bound.pos && !exit {
            self.unmark(node.container.node, upper_bound);
        }
    }

    pub fn free(&self, start: usize, pages: usize) {
        let level = self.tree.height() - pages.ilog2() as usize;
        let level_offset = (self.num_pages / (1 << (level - 1))) * PAGE_SIZE;

        // println!("start {}", start);
        // println!("level_offset {}", level_offset);
        // println!("level {}", level);
        // println!(
        //     "pos {}",
        //     (1 << (level - 1)) + (start - self.start) / level_offset
        // );

        self.free_node(
            self.tree
                .node((1 << (level - 1)) + (start - self.start) / level_offset),
            self.tree.root(),
        );
    }

    fn lock_descendants(&self, node: &Node, mut val: usize) -> usize {
        if node.pos as usize * 2 >= self.tree.node_count() {
            return val;
        }

        if self.tree.left_of(node).container_pos < 8 {
            val = Self::lock_not_leaf(val, self.tree.left_of(node).container_pos);
            val = Self::lock_not_leaf(val, self.tree.right_of(node).container_pos);

            val = self.lock_descendants(self.tree.left_of(node), val);
            val = self.lock_descendants(self.tree.right_of(node), val);
        } else {
            val = Self::lock_leaf(val, self.tree.left_of(node).container_pos);
            val = Self::lock_leaf(val, self.tree.right_of(node).container_pos);
        }

        val
    }

    fn check_parent(&self, node: &Node) -> Option<usize> {
        let mut parent = self.tree.parent_of(node);
        let root = parent.container.node;

        while {
            let mut new_val;

            new_val = parent.container.nodes.load(Ordering::Relaxed);

            let old_val = new_val;

            if Self::is_occupied(old_val, parent.container_pos) {
                return Some(parent.pos as usize);
            }

            if self.tree.node(parent.pos as usize * 2).pos == node.pos {
                new_val = Self::clean_left_coalesce(new_val, parent.container_pos);
                new_val = Self::occupy_left(new_val, parent.container_pos);
            } else {
                new_val = Self::clean_rigth_coalesce(new_val, parent.container_pos);
                new_val = Self::occupy_rigth(new_val, parent.container_pos);
            }

            new_val = Self::lock_not_leaf(new_val, self.tree.parent_of(parent).container_pos);
            parent = self.tree.parent_of(parent);
            new_val = Self::lock_not_leaf(new_val, self.tree.parent_of(parent).container_pos);
            new_val = Self::lock_not_leaf(new_val, root.container_pos);

            self.tree
                .parent_of(node)
                .container
                .nodes
                .compare_exchange(old_val, new_val, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        } {}

        if root.pos == 1 {
            None
        } else {
            self.check_parent(root)
        }
    }

    fn try_alloc_node(&self, node: &Node) -> Option<usize> {
        while {
            let mut new_val;
            let old_val;

            new_val = node.container.nodes.load(Ordering::Relaxed);

            if !Self::is_allocable(new_val, node.container_pos) {
                return Some(node.pos as usize);
            }

            old_val = new_val;

            let root_pos = node.container.node.pos;
            let mut cur = node;

            while cur.pos != root_pos {
                new_val = Self::lock_not_leaf(new_val, self.tree.parent_of(cur).container_pos);

                cur = self.tree.parent_of(cur);
            }

            if node.container_pos >= 8 {
                new_val = Self::lock_leaf(new_val, node.container_pos);
            } else {
                new_val = Self::lock_not_leaf(new_val, node.container_pos);

                if node.pos as usize * 2 < self.tree.node_count() {
                    new_val = self.lock_descendants(node, new_val);
                }
            }

            node.container
                .nodes
                .compare_exchange(old_val, new_val, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        } {}

        if node.container.node.pos == 1 {
            None
        } else {
            self.check_parent(node.container.node)
        }
    }
}

unsafe impl<'a, const PAGE_SIZE: usize, C: Cpu, A: Allocator> Send
    for BuddyAlloc<'a, PAGE_SIZE, C, A>
{
}
unsafe impl<'a, const PAGE_SIZE: usize, C: Cpu, A: Allocator> Sync
    for BuddyAlloc<'a, PAGE_SIZE, C, A>
{
}
