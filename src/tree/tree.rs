use core::alloc::{Allocator, Layout};
use core::mem::{align_of, size_of};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::tree::cpuid::Cpu;
use core::marker::PhantomData;

const COALESCE_LEFT: usize = 0x8;
const COALESCE_RIGHT: usize = 0x4;

struct NodeContainer<'a> {
    nodes: AtomicUsize,
    node: &'a Node<'a>,
}

struct Node<'a> {
    start: usize,
    size: usize,
    pos: u32,
    container_pos: u8,
    container: &'a NodeContainer<'a>,
}

pub struct Tree<'a, const PAGE_SIZE: usize, const NUM_CPUS: usize, C: Cpu> {
    tree: &'a mut [Node<'a>],
    container: &'a mut [NodeContainer<'a>],
    start: usize,
    size: usize,
    height: usize,
    num_pages: usize,
    _d: PhantomData<C>,
}

macro_rules! NODE {
    ($x:expr, $y:expr) => {
        $x.tree[$y]
    };
}

macro_rules! ROOT {
    ($x:expr) => {
        NODE!($x, 1)
    };
}

macro_rules! PARENT {
    ($x:expr, $y:expr) => {
        NODE!($x, $y as usize / 2)
    };
}

macro_rules! LEFT {
    ($x:expr, $y:expr) => {
        NODE!($x, $y as usize * 2)
    };
}

macro_rules! RIGHT {
    ($x:expr, $y:expr) => {
        NODE!($x, ($y as usize * 2) + 1)
    };
}

impl<'a, const PAGE_SIZE: usize, const NUM_CPUS: usize, C: Cpu> Tree<'a, PAGE_SIZE, NUM_CPUS, C> {
    #[inline]
    fn num_nodes(&self) -> usize {
        self.num_pages * 2 - 1
    }

    fn allocate_space<A: Allocator>(
        pages: usize,
        backend: &A,
    ) -> Option<(&mut [Node], &mut [NodeContainer])> {
        let num_pages = pages.next_power_of_two();
        let nodes_count = num_pages * 2 - 1;

        let tree_layout =
            Layout::from_size_align((nodes_count + 1) * size_of::<Node>(), align_of::<Node>())
                .ok()?;

        let con_layout = Layout::from_size_align(
            (nodes_count + 1) * size_of::<NodeContainer>(),
            align_of::<Node>(),
        )
        .ok()?;

        let tree = backend.allocate_zeroed(tree_layout).ok()?;

        let tree = unsafe {
            core::slice::from_raw_parts_mut(
                tree.as_ptr().as_mut_ptr() as *mut Node,
                nodes_count + 1,
            )
        };

        let container = backend.allocate_zeroed(con_layout).ok()?;

        let container = unsafe {
            core::slice::from_raw_parts_mut(
                container.as_ptr().as_mut_ptr() as *mut NodeContainer,
                nodes_count + 1,
            )
        };

        Some((tree, container))
    }

    unsafe fn init_tree(
        tree: *mut Node<'a>,
        nodes: *mut NodeContainer<'a>,
        size: usize,
        num_pages: usize,
        height: usize,
    ) {
        let mut container_num = 0;
        let root = tree.offset(1).as_mut().unwrap();

        root.start = 0;
        root.size = size;
        root.pos = 1;
        root.container_pos = 1;

        let mut node = nodes.offset(container_num).as_mut().unwrap();

        node.node = tree.offset(1).as_ref().unwrap();
        root.container = nodes.offset(container_num).as_ref().unwrap();

        container_num += 1;

        for i in 2..num_pages * 2 {
            let node = tree.offset(i as isize).as_mut().unwrap();
            let parent = tree.offset(i as isize / 2).as_ref().unwrap();

            node.pos = i as u32;
            node.size = parent.size / 2;

            if (height - (node.size / PAGE_SIZE).ilog2() as usize) % 4 == 1 {
                let mut n = nodes.offset(container_num).as_mut().unwrap();

                n.node = node;

                tree.offset(i as isize).as_mut().unwrap().container =
                    nodes.offset(container_num).as_ref().unwrap();
                container_num += 1;

                tree.offset(i as isize).as_mut().unwrap().container_pos = 1;
            } else {
                node.container = parent.container;

                if parent.pos * 2 == i as u32 {
                    node.container_pos = parent.container_pos * 2;
                } else {
                    node.container_pos = parent.container_pos * 2 + 1;
                }
            }

            if parent.pos as usize * 2 == i {
                tree.offset(i as isize).as_mut().unwrap().start = parent.start;
            } else {
                tree.offset(i as isize).as_mut().unwrap().start = parent.start + node.size;
            }
        }

        for i in 1..num_pages * 2 - 1 {
            assert!(tree.offset(i as isize).as_mut().unwrap().container_pos != 0);
            assert!(tree.offset(i as isize).as_mut().unwrap().pos != 0);
        }
    }

    pub fn new<A: Allocator>(start: usize, size: usize, backend: &'a A) -> Option<Self> {
        let mut new = core::mem::MaybeUninit::<Self>::uninit();
        let mut new_ref = unsafe { new.as_mut_ptr().as_mut().unwrap() };
        let num_pages = size.next_power_of_two();

        new_ref.start = start;

        let (tree, nodes) = Self::allocate_space(size, backend)?;

        unsafe {
            Self::init_tree(
                tree.as_mut_ptr(),
                nodes.as_mut_ptr(),
                num_pages * PAGE_SIZE,
                num_pages,
                num_pages.ilog2() as usize + 1,
            );
        }

        new_ref.container = nodes;
        new_ref.tree = tree;
        new_ref.num_pages = num_pages;
        new_ref.height = num_pages.ilog2() as usize + 1;
        new_ref.size = num_pages * PAGE_SIZE;

        unsafe { Some(new.assume_init()) }
    }

    fn dump(&self) {
        println!("Overall size {}", self.size);
        println!("Num_pages {}", self.num_pages);
        println!("Heigth {}", self.height);
        println!("Num nodes {}", self.num_nodes());
    }

    pub fn alloc(&self, pages: usize) -> Option<usize> {
        let pages = pages.next_power_of_two();
        let start_node = self.num_pages / pages;
        let last_node = (NODE!(self, start_node).pos * 2 - 1) as usize;
        let mut a = 1; //C::current_cpu() % C::cpu_count();
        let mut restared = false;

        if last_node - start_node != 0 {
            a = a % (last_node - start_node);
        } else {
            a = 0;
        }

        a += start_node;

        let started_at = a;

        while {
            match self.try_alloc_node(&NODE!(self, a)) {
                None => return Some(self.start + NODE!(self, a).start),
                Some(i) => {
                    if i == 1 {
                        return None;
                    }

                    for i in 2..self.num_pages * 2 - 1 {
                        assert!(self.tree[i].container_pos != 0);
                    }

                    a = (i + 1)
                        * (1 << (self.level(&NODE!(self, a)) - self.level(&NODE!(self, i))));
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

    #[inline]
    fn level(&self, node: &Node) -> usize {
        self.height - (node.size / PAGE_SIZE).ilog2() as usize
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
    fn clean_left_coalesce(val: usize, pos: u8) -> usize {
        val & !((COALESCE_LEFT << 7) + (5 * (pos as usize - 8)))
    }

    #[inline]
    fn clean_rigth_coalesce(val: usize, pos: u8) -> usize {
        val & !((COALESCE_RIGHT << 7) + (5 * (pos as usize - 8)))
    }

    #[inline]
    fn left_coalesce(val: usize, pos: u8) -> usize {
        val | ((COALESCE_LEFT << 7) + (5 * (pos as usize - 8)))
    }

    #[inline]
    fn rigth_coalesce(val: usize, pos: u8) -> usize {
        val | ((COALESCE_RIGHT << 7) + (5 * (pos as usize - 8)))
    }

    #[inline]
    fn occupy_left(val: usize, pos: u8) -> usize {
        val | ((0x2 << 7) + (5 * pos as usize - 8))
    }

    #[inline]
    fn occupy_rigth(val: usize, pos: u8) -> usize {
        val | ((0x1 << 7) + (5 * pos as usize - 8))
    }

    fn lock_descendants(&self, node: &Node, mut val: usize) -> usize {
        if node.pos as usize * 2 >= self.num_nodes() {
            return val;
        }

        if LEFT!(self, node.pos).container_pos < 8 {
            val = Self::lock_not_leaf(val, LEFT!(self, node.pos).container_pos);
            val = Self::lock_not_leaf(val, RIGHT!(self, node.pos).container_pos);

            val = self.lock_descendants(&LEFT!(self, node.pos), val);
            val = self.lock_descendants(&RIGHT!(self, node.pos), val);
        } else {
            val = Self::lock_leaf(val, LEFT!(self, node.pos).container_pos);
            val = Self::lock_leaf(val, RIGHT!(self, node.pos).container_pos);
        }

        val
    }

    fn check_parent(&self, node: &Node) -> Option<usize> {
        let mut parent = &PARENT!(self, node.pos);
        let root = parent.container.node;

        while {
            let mut new_val;

            new_val = parent.container.nodes.load(Ordering::Relaxed);

            let old_val = new_val;

            if Self::is_occupied(old_val, parent.container_pos) {
                return Some(parent.pos as usize);
            }

            if NODE!(self, parent.pos as usize * 2).pos == node.pos {
                new_val = Self::clean_left_coalesce(new_val, parent.container_pos);
                new_val = Self::occupy_left(new_val, parent.container_pos);
            } else {
                new_val = Self::clean_rigth_coalesce(new_val, parent.container_pos);
                new_val = Self::occupy_rigth(new_val, parent.container_pos);
            }

            new_val = Self::lock_not_leaf(new_val, PARENT!(self, parent.pos).container_pos);
            parent = &PARENT!(self, parent.pos);
            new_val = Self::lock_not_leaf(new_val, PARENT!(self, parent.pos).container_pos);
            new_val = Self::lock_not_leaf(new_val, root.container_pos);

            PARENT!(self, node.pos)
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
                new_val = Self::lock_not_leaf(new_val, PARENT!(self, cur.pos).container_pos);

                cur = &PARENT!(self, cur.pos);
            }

            if node.container_pos >= 8 {
                new_val = Self::lock_leaf(new_val, node.container_pos);
            } else {
                new_val = Self::lock_not_leaf(new_val, node.container_pos);

                if node.pos as usize * 2 < self.num_nodes() {
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
