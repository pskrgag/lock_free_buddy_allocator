use core::alloc::{Allocator, Layout};
use core::mem::{align_of, size_of};
use core::sync::atomic::AtomicUsize;

pub struct NodeContainer<'a> {
    pub nodes: AtomicUsize,
    pub node: &'a Node<'a>,
}

pub struct Node<'a> {
    pub start: usize,
    pub size: usize,
    pub pos: u32,
    pub container_pos: u8,
    pub container: &'a NodeContainer<'a>,
}

pub struct Tree<'a, const PAGE_SIZE: usize, A: Allocator> {
    tree: &'a mut [Node<'a>],
    container: &'a mut [NodeContainer<'a>],
    height: usize,
    num_nodes: usize,
    backend: &'a A,
}

impl<'a> PartialEq for Node<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.pos == other.pos
    }
}

impl<'a, const PAGE_SIZE: usize, A: Allocator> Tree<'a, PAGE_SIZE, A> {
    fn allocate_space(pages: usize, backend: &A) -> Option<(&mut [Node], &mut [NodeContainer])> {
        let num_pages = pages.next_power_of_two();
        let nodes_count = num_pages * 2 - 1;

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
                    node.container_pos = (parent.container_pos * 2) + 1;
                }
            }

            if parent.pos as usize * 2 == i {
                tree.offset(i as isize).as_mut().unwrap().start = parent.start;
            } else {
                tree.offset(i as isize).as_mut().unwrap().start = parent.start + node.size;
            }
        }

        for i in 1..num_pages * 2 {
            assert!(tree.offset(i as isize).as_mut().unwrap().container_pos != 0);
            assert!(tree.offset(i as isize).as_mut().unwrap().pos != 0);

            // println!(
            //     "Node: pos {} offset {} level {}, cont_pos {}",
            //     tree.offset(i as isize).as_mut().unwrap().pos,
            //     tree.offset(i as isize).as_mut().unwrap().start,
            //     height
            //         - (tree.offset(i as isize).as_mut().unwrap().size / PAGE_SIZE).ilog2() as usize,
            //     tree.offset(i as isize).as_mut().unwrap().container_pos
            // );
        }
    }

    pub fn new(pages: usize, backend: &'a A) -> Option<Self> {
        let heigth = pages.ilog2() as usize + 1;
        let (tree, nodes) = Self::allocate_space(pages, backend)?;

        unsafe {
            Self::init_tree(
                tree.as_mut_ptr(),
                nodes.as_mut_ptr(),
                pages * PAGE_SIZE,
                pages,
                heigth,
            );
        }

        Some(Self {
            tree: tree,
            container: nodes,
            height: heigth,
            num_nodes: pages * 2 - 1,
            backend: backend,
        })
    }

    #[inline]
    pub fn height(&self) -> usize {
        self.height
    }

    #[inline]
    pub fn node_count(&self) -> usize {
        self.num_nodes
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

impl<'a, const PAGE_SIZE: usize, A: Allocator> Drop for Tree<'_, PAGE_SIZE, A> {
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
