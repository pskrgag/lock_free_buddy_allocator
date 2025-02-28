//! Scalable lock-free buddy system allocator implementation
//!
//! Algorithm is based on [Andrea Scarselli's thesis](https://alessandropellegrini.it/publications/tScar17.pdf).

use core::alloc::Allocator;
use core::sync::atomic::Ordering;

use crate::cpuid::Cpu;
use crate::tree::{Node, Tree};
use core::marker::PhantomData;

const COALESCE_LEFT: usize = 0x8;
const COALESCE_RIGHT: usize = 0x4;

/// Lock-free buddy system allocator.
///
/// Buddy system allocator allows to allocate chunks with size of power of two.
///
/// `PAGE_SIZE` generic parameter defines the minimum size of a chunk to be allocated. `C` defines
/// an interface for obtaining ID of current CPU, which is used for routing different CPUs to
/// different part of the allocator to prevent contention. `A` is a back-end allocator used for
/// internal data allocations.
pub struct BuddyAlloc<'a, C: Cpu, A: Allocator + 'a, const PAGE_SIZE: usize = 4096>
{
    tree: Tree<'a, PAGE_SIZE, A>,
    start: usize,
    num_pages: usize,
    _d: PhantomData<C>,
}

impl<'a, const PAGE_SIZE: usize, C: Cpu, A: Allocator + 'a> BuddyAlloc<'a, C, A, PAGE_SIZE>
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
        val | (0x13 << (7 + (5 * (pos as usize - 8))))
    }

    #[inline]
    fn unlock_not_leaf(val: usize, pos: u8) -> usize {
        val & !(0x1 << (pos as usize - 1))
    }

    #[inline]
    fn unlock_leaf(val: usize, pos: u8) -> usize {
        val & !(0x13 << (7 + (5 * (pos as usize - 8))))
    }

    #[inline]
    fn clean_left_coalesce(val: usize, pos: u8) -> usize {
        val & !(COALESCE_LEFT << (7 + (5 * (pos as usize - 8))))
    }

    #[inline]
    fn clean_rigth_coalesce(val: usize, pos: u8) -> usize {
        val & !(COALESCE_RIGHT << (7 + (5 * (pos as usize - 8))))
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

    /// Creates new buddy allocator.
    ///
    /// Allocator works on top of address range, (start..num_pages * PAGE_SIZE). Number of pages
    /// should be power of two, otherwise function will return failure. `backend` will be used for
    /// internal allocations.
    pub fn new(start: usize, num_pages: usize, backend: &'a A) -> Option<Self> {
        if !num_pages.is_power_of_two() {
            return None;
        }

        Some(Self {
            tree: Tree::<PAGE_SIZE, A>::new(num_pages, backend)?,
            num_pages,
            start,
            _d: PhantomData,
        })
    }

    /// Allocates pages
    ///
    /// Function allocates `1 << order` number of pages. On success return address of the start of
    /// the region, otherwise returns None indicating out-of-memory situation
    pub fn alloc(&self, order: usize) -> Option<usize> {
        let pages = 1 << order;
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
                    return Some(self.start + self.tree.node(a).start);
                }
                Some(i) => {
                    if i == 1 {
                        return None;
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
        let r_parent = self.tree.right_of(parent);

        if l_parent == node && !Self::is_allocable(val, r_parent.container_pos) {
            true
        } else if r_parent == node && !Self::is_allocable(val, l_parent.container_pos) {
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

    fn unmark(&self, node: &Node, upper_bound: &Node) {
        let mut exit;
        let mut cur;

        'foo: while {
            let parent = self.tree.parent_of(node);
            let mut new_val = parent.container.nodes.load(Ordering::Relaxed);
            let old_val = new_val;

            cur = node;
            exit = false;

            if self.tree.left_of(parent) == node {
                if !Self::is_left_coalescing(new_val, parent.container_pos) {
                    return;
                }

                new_val = Self::clean_left_coalesce(new_val, parent.container_pos);
                new_val = Self::clean_left(new_val, parent.container_pos);

                if Self::is_occupied_rigth(new_val, parent.container_pos) {
                    if parent
                        .container
                        .nodes
                        .compare_exchange(old_val, new_val, Ordering::Relaxed, Ordering::Relaxed)
                        .is_err()
                    {
                        break 'foo;
                    } else {
                        continue 'foo;
                    }
                }
            }

            if self.tree.right_of(parent) == node {
                if !Self::is_right_coalescing(new_val, parent.container_pos) {
                    return;
                }

                new_val = Self::clean_rigth_coalesce(new_val, parent.container_pos);
                new_val = Self::clean_rigth(new_val, parent.container_pos);

                if Self::is_occupied_left(new_val, parent.container_pos) {
                    if parent
                        .container
                        .nodes
                        .compare_exchange(old_val, new_val, Ordering::Relaxed, Ordering::Relaxed)
                        .is_err()
                    {
                        continue 'foo;
                    } else {
                        break 'foo;
                    }
                }
            }

            cur = self.tree.parent_of(node);

            'bar: while cur.pos != cur.container.node.pos {
                exit = self.check_brother(cur, new_val);
                if exit {
                    break 'bar;
                }

                new_val = Self::unlock_not_leaf(new_val, self.tree.parent_of(cur).container_pos);
                cur = self.tree.parent_of(cur);
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

    fn mark(&self, node: &Node, upper_bound: &Node) {
        let parent = self.tree.parent_of(node);

        while {
            let mut new_val = parent.container.nodes.load(Ordering::Relaxed);
            let old_val = new_val;

            if self.tree.left_of(parent) == node {
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

    fn free_node(&self, node: &Node, upper_bound: &Node) {
        let mut exit;

        if node.container.node.pos != upper_bound.pos {
            self.mark(node.container.node, upper_bound);
        }

        while {
            let mut new_val = node.container.nodes.load(Ordering::Relaxed);
            let old_val = new_val;
            let mut cur = node;

            exit = false;

            'inner: while cur.pos != node.container.node.pos {
                exit = self.check_brother(cur, new_val);
                if exit {
                    break 'inner;
                }

                new_val = Self::unlock_not_leaf(new_val, self.tree.parent_of(cur).container_pos);
                cur = self.tree.parent_of(cur);
            }

            if !self.tree.is_leaf(node) && node.pos as usize * 2 <= self.tree.node_count() {
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

    /// Frees previously allocated pages.
    ///
    /// Function frees `1 << order` pages starting from `start`.
    pub fn free(&self, start: usize, order: usize) -> Option<()> {
        if order > self.tree.height() {
            return None;
        }

        let level = self.tree.height() - order;
        let level_offset = (self.num_pages / (1 << (level - 1))) * PAGE_SIZE;

        self.free_node(
            self.tree
                .node((1 << (level - 1)) + (start - self.start) / level_offset),
            self.tree.root(),
        );
        Some(())
    }

    fn lock_descendants(&self, node: &Node, mut val: usize) -> usize {
        if node.pos as usize * 2 >= self.tree.node_count() {
            return val;
        }

        if !self.tree.is_leaf(self.tree.left_of(node)) {
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

    fn check_parent(&self, node: &Node) -> Option<(usize, usize)> {
        let mut parent = self.tree.parent_of(node);
        let root = parent.container.node;

        while {
            let mut new_val;

            new_val = parent.container.nodes.load(Ordering::Relaxed);

            let old_val = new_val;

            if Self::is_occupied(new_val, parent.container_pos) {
                return Some((parent.pos as usize, node.pos as usize));
            }

            if self.tree.left_of(parent) == node {
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

        if root == self.tree.root() {
            None
        } else {
            self.check_parent(root)
        }
    }

    #[cfg(test)]
    pub(crate) fn __try_alloc_node(&self, pos: usize) -> Option<usize> {
        self.try_alloc_node(self.tree.node(pos))
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

            if self.tree.is_leaf(node) {
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

        if node.container.node == self.tree.root() {
            return None;
        }

        match self.check_parent(node.container.node) {
            None => None,
            Some((i, n)) => {
                self.free_node(node, self.tree.node(n));
                Some(i)
            }
        }
    }
}

unsafe impl<'a, C: Cpu, A: Allocator, const PAGE_SIZE: usize> Send
    for BuddyAlloc<'a, C, A, PAGE_SIZE>
{
}
unsafe impl<'a, C: Cpu, A: Allocator, const PAGE_SIZE: usize> Sync
    for BuddyAlloc<'a, C, A, PAGE_SIZE>
{
}
