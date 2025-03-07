use core::ops::Deref;

const COALESCE_LEFT: usize = 0x8;
const COALESCE_RIGHT: usize = 0x4;

const LEFT_OCCUPIED: usize = 0x2;
const RIGHT_OCCUPIED: usize = 0x1;

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(transparent)]
pub struct NodeState(usize);

impl NodeState {
    fn is_leaf(pos: u8) -> bool {
        pos >= 8
    }

    fn leaf_offset(pos: u8) -> usize {
        debug_assert!(pos >= 8);
        // 7 bits for FREE / OCCUPIED state in non-leaf nodes and 5 bits per-leaf node
        7 + (5 * (pos as usize - 8))
    }

    pub fn is_allocable(&self, pos: u8) -> bool {
        if pos < 8 {
            (self.0 & (0x1 << (pos - 1))) == 0
        } else {
            self.0 & ((0x1F << 7) << (5 * (pos - 8))) == 0
        }
    }

    pub fn lock_not_leaf(&self, pos: u8) -> Self {
        (self.0 | (0x1 << (pos as usize - 1))).into()
    }

    pub fn lock_leaf(&self, pos: u8) -> Self {
        (self.0 | (0x13 << Self::leaf_offset(pos))).into()
    }

    pub fn unlock(&self, pos: u8) -> Self {
        if Self::is_leaf(pos) {
            self.unlock_leaf(pos)
        } else {
            self.unlock_not_leaf(pos)
        }
    }

    pub fn unlock_not_leaf(&self, pos: u8) -> Self {
        (self.0 & !(0x1 << (pos as usize - 1))).into()
    }

    fn unlock_leaf(&self, pos: u8) -> Self {
        (self.0 & !(0x13 << Self::leaf_offset(pos))).into()
    }

    pub fn is_occupied(&self, pos: u8) -> bool {
        if pos < 8 {
            (self.0 & (0x1 << (pos - 1))) != 0
        } else {
            self.0 & ((0x1 << 6) << (5 * (pos - 7))) != 0
        }
    }

    pub fn clean_left_coalesce(&self, pos: u8) -> Self {
        (self.0 & !(COALESCE_LEFT << Self::leaf_offset(pos))).into()
    }

    pub fn clean_rigth_coalesce(&self, pos: u8) -> Self {
        (self.0 & !(COALESCE_RIGHT << Self::leaf_offset(pos))).into()
    }

    pub fn left_coalesce(&self, pos: u8) -> Self {
        (self.0 | (COALESCE_LEFT << Self::leaf_offset(pos))).into()
    }

    pub fn rigth_coalesce(&self, pos: u8) -> Self {
        (self.0 | (COALESCE_RIGHT << Self::leaf_offset(pos))).into()
    }

    pub fn occupy_left(&self, pos: u8) -> Self {
        (self.0 | (LEFT_OCCUPIED << Self::leaf_offset(pos))).into()
    }

    pub fn occupy_rigth(&self, pos: u8) -> Self {
        (self.0 | (RIGHT_OCCUPIED << Self::leaf_offset(pos))).into()
    }

    pub fn is_left_coalescing(&self, pos: u8) -> bool {
        *self == self.left_coalesce(pos)
    }

    pub fn is_right_coalescing(&self, pos: u8) -> bool {
        *self == self.rigth_coalesce(pos)
    }

    pub fn clean_left_occupy(&self, pos: u8) -> Self {
        (self.0 & !(LEFT_OCCUPIED << Self::leaf_offset(pos))).into()
    }

    pub fn clean_rigth_occupy(&self, pos: u8) -> Self {
        (self.0 & !(RIGHT_OCCUPIED << Self::leaf_offset(pos))).into()
    }

    pub fn is_occupied_rigth(&self, pos: u8) -> bool {
        *self == self.occupy_rigth(pos)
    }

    pub fn is_occupied_left(&self, pos: u8) -> bool {
        *self == self.occupy_left(pos)
    }
}

impl From<usize> for NodeState {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl Deref for NodeState {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
