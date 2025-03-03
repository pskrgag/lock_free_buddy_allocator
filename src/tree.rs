use super::state::NodeState;
use crate::{AtomicUsize, Ordering};
use core::alloc::{Allocator, Layout};
use core::mem::{align_of, size_of};

#[derive(Debug)]
pub(crate) struct NodeContainer {
    // State of 15 nodes
    nodes: AtomicUsize,
    // Root of the sub-tree
    pub root: u32,
}

impl NodeContainer {
    pub fn get_state(&self) -> NodeState {
        self.nodes.load(Ordering::Relaxed).into()
    }

    pub fn try_update(&self, old: NodeState, current: NodeState) -> bool {
        self.nodes
            .compare_exchange(*old, *current, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }
}

/// The node of the binary tree
#[derive(Debug)]
pub(crate) struct Node {
    // Compressed order and container position
    order_and_pos: u8,
    // Start of the region
    pub start: usize,
    // Position in the binary tree
    pub pos: u32,
    // Container with node state
    pub container_offset: u32,
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.pos == other.pos
    }
}

impl Node {
    pub fn order(&self) -> usize {
        self.order_and_pos as usize & 0xf
    }

    pub fn container_pos(&self) -> u8 {
        (self.order_and_pos >> 4) + 1
    }

    fn set_order_and_pos(&mut self, order: u8, container_pos: u8) {
        let container_pos = container_pos - 1;

        debug_assert!(order <= 0xf);
        debug_assert!(container_pos <= 0xf);

        self.order_and_pos = order | (container_pos << 4);
    }
}

pub(crate) struct Tree<'a, A: Allocator> {
    tree: &'a mut [Node],
    container: &'a mut [NodeContainer],
    order: u8,
    backend: &'a A,
}

impl<'a, A: Allocator> Tree<'a, A> {
    pub fn container(&self, offset: u32) -> &NodeContainer {
        &self.container[offset as usize]
    }

    pub fn node(&self, offset: u32) -> &Node {
        &self.tree[offset as usize]
    }

    fn num_nodes_from_order(order: u8) -> usize {
        (1 << order) * 2 - 1
    }

    fn allocate_space(order: u8, backend: &A) -> Option<(&mut [Node], &mut [NodeContainer])> {
        let nodes_count = Self::num_nodes_from_order(order);

        let tree_layout =
            Layout::from_size_align((nodes_count + 1) * size_of::<Node>(), align_of::<Node>())
                .ok()?;

        let con_layout = Layout::from_size_align(
            (nodes_count - 1) * size_of::<NodeContainer>(),
            align_of::<NodeContainer>(),
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
                nodes_count - 1,
            )
        };

        Some((tree, container))
    }

    fn init_tree(tree: &mut [Node], nodes: &mut [NodeContainer], order: u8) {
        let height = order + 1;
        let mut container_num = 0;

        for i in nodes.iter_mut() {
            i.nodes = AtomicUsize::new(0);
        }

        tree[1].start = 0;
        tree[1].pos = 1;
        tree[1].set_order_and_pos(order, 1);

        nodes[container_num].root = 1;
        tree[1].container_offset = container_num as u32;

        container_num += 1;

        for i in 2..Self::num_nodes_from_order(order) + 1 {
            let order = tree[i / 2].order() as u8 - 1;

            tree[i].pos = i as u32;

            if (height - order) % 4 == 1 {
                nodes[container_num].root = i as u32;
                tree[i].container_offset = container_num as u32;
                container_num += 1;

                tree[i].set_order_and_pos(order, 1);
            } else {
                tree[i].container_offset = tree[i / 2].container_offset;

                if tree[i / 2].pos * 2 == i as u32 {
                    tree[i].set_order_and_pos(order, tree[i / 2].container_pos() * 2);
                } else {
                    tree[i].set_order_and_pos(order, tree[i / 2].container_pos() * 2 + 1);
                }
            }

            if tree[i / 2].pos as usize * 2 == i {
                tree[i].start = tree[i / 2].start;
            } else {
                tree[i].start = tree[i / 2].start + (1 << tree[i].order());
            }
        }

        for i in 1..Self::num_nodes_from_order(order) {
            debug_assert!(tree[i].container_pos() != 0);
            debug_assert!(tree[i].pos != 0);
        }
    }

    pub fn new(order: u8, backend: &'a A) -> Option<Self> {
        let (tree, container) = Self::allocate_space(order, backend)?;

        Self::init_tree(tree, container, order);

        Some(Self {
            tree,
            container,
            order,
            backend,
        })
    }

    #[inline]
    pub fn height(&self) -> usize {
        (self.order + 1) as usize
    }

    #[inline]
    pub fn node_count(&self) -> usize {
        Self::num_nodes_from_order(self.order)
    }

    #[inline]
    pub fn root(&self) -> &Node {
        &self.tree[1]
    }

    #[inline]
    pub fn parent_of(&self, node: &Node) -> &Node {
        &self.tree[node.pos as usize / 2]
    }

    #[inline]
    pub fn left_of(&self, node: &Node) -> &Node {
        &self.tree[node.pos as usize * 2]
    }

    #[inline]
    pub fn right_of(&self, node: &Node) -> &Node {
        &self.tree[node.pos as usize * 2 + 1]
    }

    pub fn is_leaf(&self, node: &Node) -> bool {
        node.container_pos() >= 8
    }
}

impl<A: Allocator> Drop for Tree<'_, A> {
    fn drop(&mut self) {
        use core::ptr::NonNull;

        let nodes_count = self.node_count();

        let tree_layout =
            Layout::from_size_align((nodes_count + 1) * size_of::<Node>(), align_of::<Node>())
                .unwrap();

        let con_layout = Layout::from_size_align(
            (nodes_count - 1) * size_of::<NodeContainer>(),
            align_of::<NodeContainer>(),
        )
        .unwrap();

        unsafe {
            let slice = NonNull::new(self.tree.as_mut_ptr() as *mut u8).unwrap();
            self.backend.deallocate(slice, tree_layout);

            let slice = NonNull::new(self.container.as_mut_ptr() as *mut u8).unwrap();
            self.backend.deallocate(slice, con_layout);
        }
    }
}
