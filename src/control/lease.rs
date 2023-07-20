//! DRM Lease related structures

/// A DRM lessee id
#[repr(transparent)]
#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug)]
pub struct LesseeId(u32);

impl From<LesseeId> for u32 {
    fn from(handle: LesseeId) -> Self {
        handle.0.into()
    }
}

impl From<u32> for LesseeId {
    fn from(handle: u32) -> Self {
        LesseeId(handle)
    }
}
