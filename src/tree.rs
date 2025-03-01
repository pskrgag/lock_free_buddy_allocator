use core::alloc::{Allocator, Layout};
use core::mem::{align_of, size_of};
use core::sync::atomic::AtomicUsize;

#[derive(Debug)]
pub(crate) struct NodeContainer<'a> {
    pub nodes: AtomicUsize,
    pub node: &'a Node<'a>,
}

#[derive(Debug)]
pub(crate) struct Node<'a> {
    pub start: usize,
    pub order: u8,
    pub pos: u32,
    pub container_pos: u8,
    pub container: &'a NodeContainer<'a>,
}

pub(crate) struct Tree<'a, A: Allocator> {
    pub tree: &'a mut [Node<'a>],
    container: &'a mut [NodeContainer<'a>],
    order: u8,
    backend: &'a A,
}

impl PartialEq for Node<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.pos == other.pos
    }
}

impl<'a, A: Allocator> Tree<'a, A> {
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

    unsafe fn init_tree(tree: *mut Node<'a>, nodes: *mut NodeContainer<'a>, order: u8) {
        let height = order + 1;
        let mut container_num = 0;
        let root = tree.offset(1).as_mut().unwrap();

        root.start = 0;
        root.order = order;
        root.pos = 1;
        root.container_pos = 1;

        let node = nodes.offset(container_num).as_mut().unwrap();

        node.node = tree.offset(1).as_ref().unwrap();
        root.container = nodes.offset(container_num).as_ref().unwrap();

        container_num += 1;

        for i in 2..Self::num_nodes_from_order(order) + 1 {
            let node = tree.add(i).as_mut().unwrap();
            let parent = tree.add(i / 2).as_ref().unwrap();

            node.pos = i as u32;
            node.order = parent.order - 1;

            if (height - node.order) % 4 == 1 {
                let n = nodes.offset(container_num).as_mut().unwrap();

                n.node = node;

                tree.add(i).as_mut().unwrap().container =
                    nodes.offset(container_num).as_ref().unwrap();
                container_num += 1;

                tree.add(i).as_mut().unwrap().container_pos = 1;
            } else {
                node.container = parent.container;

                if parent.pos * 2 == i as u32 {
                    node.container_pos = parent.container_pos * 2;
                } else {
                    node.container_pos = (parent.container_pos * 2) + 1;
                }
            }

            if parent.pos as usize * 2 == i {
                tree.add(i).as_mut().unwrap().start = parent.start;
            } else {
                tree.add(i).as_mut().unwrap().start = parent.start + (1 << node.order as usize);
            }
        }

        for i in 1..Self::num_nodes_from_order(order) {
            debug_assert!(tree.add(i).as_mut().unwrap().container_pos != 0);
            debug_assert!(tree.add(i).as_mut().unwrap().pos != 0);
        }
    }

    pub fn new(order: u8, backend: &'a A) -> Option<Self> {
        let (tree, nodes) = Self::allocate_space(order, backend)?;

        unsafe {
            Self::init_tree(tree.as_mut_ptr(), nodes.as_mut_ptr(), order);
        }

        Some(Self {
            tree,
            container: nodes,
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
    pub fn node(&self, pos: usize) -> &Node {
        &self.tree[pos]
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
        node.container_pos >= 8
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
