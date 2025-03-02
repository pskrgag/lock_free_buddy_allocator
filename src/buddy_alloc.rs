//! Core allocator structure

use core::alloc::Allocator;

use super::state::NodeState;
use crate::cpuid::Cpu;
use crate::tree::{Node, Tree};
use core::marker::PhantomData;

/// Lock-free buddy system allocator.
///
/// Buddy system allocator allows to allocate chunks with size of power of two.
///
/// `PAGE_SIZE` generic parameter defines the size of the minimum chunk to be allocated. `C` defines
/// an interface for obtaining ID of current CPU, which is used for routing different CPUs to
/// different part of the allocator to prevent contention. `A` is a back-end allocator used for
/// internal data allocations.
pub struct BuddyAlloc<'a, C: Cpu, A: Allocator + 'a, const PAGE_SIZE: usize = 4096> {
    tree: Tree<'a, A>,
    start: usize,
    order: u8,
    _d: PhantomData<C>,
}

impl<'a, const PAGE_SIZE: usize, C: Cpu, A: Allocator + 'a> BuddyAlloc<'a, C, A, PAGE_SIZE> {
    #[inline]
    fn level(&self, node: &Node) -> usize {
        self.tree.height() - node.order()
    }

    /// Creates new buddy allocator.
    ///
    /// Allocator works on top of address range, (start..num_pages * PAGE_SIZE). Number of pages
    /// should be power of two, otherwise function will return failure. `backend` will be used for
    /// internal allocations.
    pub fn new(start: usize, order: u8, backend: &'a A) -> Option<Self> {
        Some(Self {
            tree: Tree::new(order, backend)?,
            order,
            start,
            _d: PhantomData,
        })
    }

    /// Allocates pages
    ///
    /// Function allocates `1 << order` number of contiguous chunks of PAGE_SIZE size.
    /// On success return address of the start of the region, otherwise returns None
    /// indicating out-of-memory situation
    pub fn alloc(&self, order: usize) -> Option<usize> {
        let start_node = 1 << (self.order as usize - order);
        let last_node = (self.tree.left_of(self.tree.node(start_node)).pos - 1) as usize;
        let mut a = C::current_cpu();
        let mut restared = false;

        if last_node - start_node != 0 {
            a %= last_node - start_node;
        } else {
            a = 0;
        }

        a += start_node;

        let started_at = a;

        while {
            debug_assert!(self.tree.node(a).order() == order);

            match self.try_alloc_node(self.tree.node(a)) {
                None => {
                    return Some((self.start + self.tree.node(a).start) * PAGE_SIZE);
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

    fn check_brother(&self, node: &Node, val: NodeState) -> bool {
        let parent = self.tree.parent_of(node);
        let l_parent = self.tree.left_of(parent);
        let r_parent = self.tree.right_of(parent);

        l_parent == node && !val.is_allocable(r_parent.container_pos())
            || r_parent == node && !val.is_allocable(l_parent.container_pos())
    }

    fn unlock_descendants(&self, node: &Node, mut val: NodeState) -> NodeState {
        if node.pos as usize * 2 >= self.tree.node_count() {
            return val;
        }

        if !self.tree.is_leaf(self.tree.left_of(node)) {
            val = val
                .lock_not_leaf(self.tree.left_of(node).container_pos())
                .lock_not_leaf(self.tree.right_of(node).container_pos());

            val = self.unlock_descendants(self.tree.left_of(node), val);
            self.unlock_descendants(self.tree.right_of(node), val)
        } else {
            val.lock_leaf(self.tree.left_of(node).container_pos())
                .lock_leaf(self.tree.right_of(node).container_pos())
        }
    }

    fn unmark(&self, node: &Node, upper_bound: &Node) {
        let mut exit;
        let mut cur;

        'foo: while {
            let parent = self.tree.parent_of(node);
            let container = self.tree.container(parent.container_offset);
            let mut new_val = container.get_state();
            let old_val = new_val;

            cur = node;
            exit = false;

            if self.tree.left_of(parent) == node {
                if !new_val.is_left_coalescing(parent.container_pos()) {
                    return;
                }

                new_val = new_val
                    .clean_left_coalesce(parent.container_pos())
                    .clean_left_occupy(parent.container_pos());

                if new_val.is_occupied_rigth(parent.container_pos()) {
                    if !container.try_update(old_val, new_val) {
                        continue 'foo;
                    } else {
                        break 'foo;
                    }
                }
            }

            if self.tree.right_of(parent) == node {
                if !new_val.is_right_coalescing(parent.container_pos()) {
                    return;
                }

                new_val = new_val
                    .clean_rigth_coalesce(parent.container_pos())
                    .clean_rigth_occupy(parent.container_pos());

                if new_val.is_occupied_left(parent.container_pos()) {
                    if !container.try_update(old_val, new_val) {
                        continue 'foo;
                    } else {
                        break 'foo;
                    }
                }
            }

            cur = self.tree.parent_of(node);
            let cur_cont = self.tree.container(cur.container_offset);

            while cur.pos != cur_cont.root().pos {
                exit = self.check_brother(cur, new_val);
                if exit {
                    break;
                }

                new_val = new_val.unlock_not_leaf(self.tree.parent_of(cur).container_pos());
                cur = self.tree.parent_of(cur);
            }

            !container.try_update(old_val, new_val)
        } {}

        if cur.pos != upper_bound.pos && !exit {
            self.unmark(cur, upper_bound)
        }
    }

    fn mark(&self, node: &Node, upper_bound: &Node) {
        let parent = self.tree.parent_of(node);
        let container = self.tree.container(parent.container_offset);

        while {
            let mut new_val = container.get_state();
            let old_val = new_val;

            new_val = if self.tree.left_of(parent) == node {
                new_val.left_coalesce(parent.container_pos())
            } else {
                new_val.rigth_coalesce(parent.container_pos())
            };

            !container.try_update(old_val, new_val)
        } {}

        if container.root().pos != upper_bound.pos {
            self.mark(container.root(), upper_bound);
        }
    }

    fn free_node(&self, node: &Node, upper_bound: &Node) {
        let mut exit;
        let container = self.tree.container(node.container_offset);

        if container.root().pos != upper_bound.pos {
            self.mark(container.root(), upper_bound);
        }

        while {
            let mut new_val = container.get_state();
            let old_val = new_val;
            let mut cur = node;

            exit = false;

            'inner: while cur.pos != container.root().pos {
                exit = self.check_brother(cur, new_val);
                if exit {
                    break 'inner;
                }

                new_val = new_val.unlock_not_leaf(self.tree.parent_of(cur).container_pos());
                cur = self.tree.parent_of(cur);
            }

            if !self.tree.is_leaf(node) && node.pos as usize * 2 <= self.tree.node_count() {
                new_val = self.unlock_descendants(node, new_val);
            }

            new_val = new_val.unlock(node.container_pos());
            !container.try_update(old_val, new_val)
        } {}

        if container.root().pos != upper_bound.pos && !exit {
            self.unmark(container.root(), upper_bound);
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
        let level_offset = (1 << (self.order as usize - level + 1)) * PAGE_SIZE;

        self.free_node(
            self.tree
                .node((1 << (level - 1)) + (start - self.start) / level_offset),
            self.tree.root(),
        );
        Some(())
    }

    fn lock_descendants(&self, node: &Node, mut val: NodeState) -> NodeState {
        if node.pos as usize * 2 >= self.tree.node_count() {
            return val;
        }

        debug_assert!(node.order() != 0);
        debug_assert!(
            self.tree.is_leaf(self.tree.left_of(node))
                == self.tree.is_leaf(self.tree.right_of(node))
        );

        if !self.tree.is_leaf(self.tree.left_of(node)) {
            val = val
                .lock_not_leaf(self.tree.left_of(node).container_pos())
                .lock_not_leaf(self.tree.right_of(node).container_pos());

            val = self.lock_descendants(self.tree.left_of(node), val);
            self.lock_descendants(self.tree.right_of(node), val)
        } else {
            val.lock_leaf(self.tree.left_of(node).container_pos())
                .lock_leaf(self.tree.right_of(node).container_pos())
        }
    }

    fn check_parent(&self, node: &Node) -> Option<(usize, usize)> {
        let mut parent = self.tree.parent_of(node);
        let container_parent = self.tree.container(parent.container_offset);
        let root = container_parent.root();

        while {
            let mut new_val = container_parent.get_state();
            let old_val = new_val;

            if new_val.is_occupied(parent.container_pos()) {
                return Some((parent.pos as usize, node.pos as usize));
            }

            new_val = if self.tree.left_of(parent) == node {
                new_val
                    .clean_left_coalesce(parent.container_pos())
                    .occupy_left(parent.container_pos())
            } else {
                new_val
                    .clean_rigth_coalesce(parent.container_pos())
                    .occupy_rigth(parent.container_pos())
            };

            new_val = new_val.lock_not_leaf(self.tree.parent_of(parent).container_pos());
            parent = self.tree.parent_of(parent);
            new_val = new_val
                .lock_not_leaf(self.tree.parent_of(parent).container_pos())
                .lock_not_leaf(root.container_pos());

            !container_parent.try_update(old_val, new_val)
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
        debug_assert!(node.container_pos() != 0);
        let container = self.tree.container(node.container_offset);

        while {
            let mut new_val = container.get_state();

            // If node cannot be allocated -- bail out
            if !new_val.is_allocable(node.container_pos()) {
                return Some(node.pos as usize);
            }

            let old_val = new_val;
            let root_pos = container.root().pos;
            let mut cur = node;

            // Lock all nodes up to root of the container
            while cur.pos != root_pos {
                new_val = new_val.lock_not_leaf(self.tree.parent_of(cur).container_pos());

                cur = self.tree.parent_of(cur);
            }

            // Lock the node itself
            if self.tree.is_leaf(node) {
                new_val = new_val.lock_leaf(node.container_pos());
            } else {
                new_val = new_val.lock_not_leaf(node.container_pos());

                // Lock all sub-tree of children
                if node.pos as usize * 2 < self.tree.node_count() {
                    new_val = self.lock_descendants(node, new_val);
                }
            }

            // Try to commit changes
            !self
                .tree
                .container(node.container_offset)
                .try_update(old_val, new_val)
        } {}

        if container.root() == self.tree.root() {
            return None;
        }

        match self.check_parent(container.root()) {
            None => None,
            Some((i, n)) => {
                self.free_node(node, self.tree.node(n));
                Some(i)
            }
        }
    }
}

unsafe impl<C: Cpu, A: Allocator, const PAGE_SIZE: usize> Send for BuddyAlloc<'_, C, A, PAGE_SIZE> {}
unsafe impl<C: Cpu, A: Allocator, const PAGE_SIZE: usize> Sync for BuddyAlloc<'_, C, A, PAGE_SIZE> {}
