//! Shared retained byte-slice ownership for scan-time payload plumbing.

use std::{fmt, panic::RefUnwindSafe, sync::Arc};

use crate::CoveError;

/// A retained immutable byte source.
///
/// Implementations must return a stable byte slice for the lifetime of `self`.
/// This is intentionally object-safe so scan adapters can plug in owners such
/// as read-only mmap handles without making `cove-core` depend on that backend.
pub trait RetainedByteSource: fmt::Debug + RefUnwindSafe + Send + Sync + 'static {
    fn as_slice(&self) -> &[u8];
}

impl RetainedByteSource for Vec<u8> {
    fn as_slice(&self) -> &[u8] {
        self.as_slice()
    }
}

/// Owner for bytes retained by a [`RetainedBytes`] view.
#[derive(Debug, Clone)]
pub enum RetainedByteOwner {
    Vec(Arc<Vec<u8>>),
    External(Arc<dyn RetainedByteSource>),
}

impl RetainedByteOwner {
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        Self::Vec(Arc::new(bytes))
    }

    pub fn from_arc(bytes: Arc<Vec<u8>>) -> Self {
        Self::Vec(bytes)
    }

    pub fn from_external(owner: Arc<dyn RetainedByteSource>) -> Self {
        Self::External(owner)
    }

    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Vec(bytes) => bytes.as_slice(),
            Self::External(owner) => owner.as_slice(),
        }
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    fn shares_source(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Vec(left), Self::Vec(right)) => Arc::ptr_eq(left, right),
            (Self::External(left), Self::External(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

/// A cheap slice view over an owned byte allocation.
///
/// INVARIANT: `offset..offset + len` is always in bounds for `owner`; all
/// public constructors validate that range before publishing the value.
#[derive(Clone)]
pub struct RetainedBytes {
    owner: Arc<RetainedByteOwner>,
    offset: usize,
    len: usize,
}

impl RetainedBytes {
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        let len = bytes.len();
        Self {
            owner: Arc::new(RetainedByteOwner::from_vec(bytes)),
            offset: 0,
            len,
        }
    }

    pub fn from_arc(owner: Arc<Vec<u8>>) -> Self {
        let len = owner.len();
        Self {
            owner: Arc::new(RetainedByteOwner::from_arc(owner)),
            offset: 0,
            len,
        }
    }

    pub fn from_owner(owner: Arc<RetainedByteOwner>) -> Self {
        let len = owner.len();
        Self {
            owner,
            offset: 0,
            len,
        }
    }

    pub fn from_external_owner(owner: Arc<dyn RetainedByteSource>) -> Self {
        Self::from_owner(Arc::new(RetainedByteOwner::from_external(owner)))
    }

    pub fn from_arc_slice(
        owner: Arc<Vec<u8>>,
        offset: usize,
        len: usize,
    ) -> Result<Self, CoveError> {
        Self::from_owner_slice(Arc::new(RetainedByteOwner::from_arc(owner)), offset, len)
    }

    pub fn from_owner_slice(
        owner: Arc<RetainedByteOwner>,
        offset: usize,
        len: usize,
    ) -> Result<Self, CoveError> {
        let end = offset.checked_add(len).ok_or(CoveError::ArithOverflow)?;
        if end > owner.len() {
            return Err(CoveError::OffsetRange);
        }
        Ok(Self { owner, offset, len })
    }

    pub fn from_external_owner_slice(
        owner: Arc<dyn RetainedByteSource>,
        offset: usize,
        len: usize,
    ) -> Result<Self, CoveError> {
        Self::from_owner_slice(
            Arc::new(RetainedByteOwner::from_external(owner)),
            offset,
            len,
        )
    }

    pub fn slice(&self, offset: usize, len: usize) -> Result<Self, CoveError> {
        let local_end = offset.checked_add(len).ok_or(CoveError::ArithOverflow)?;
        if local_end > self.len {
            return Err(CoveError::OffsetRange);
        }
        let absolute_offset = self
            .offset
            .checked_add(offset)
            .ok_or(CoveError::ArithOverflow)?;
        Self::from_owner_slice(Arc::clone(&self.owner), absolute_offset, len)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.owner.as_slice()[self.offset..self.offset + self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn owner(&self) -> Arc<RetainedByteOwner> {
        Arc::clone(&self.owner)
    }

    pub fn owner_offset(&self) -> usize {
        self.offset
    }

    pub fn shares_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner) || self.owner.shares_source(&other.owner)
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
}

impl fmt::Debug for RetainedBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetainedBytes")
            .field("offset", &self.offset)
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

impl PartialEq for RetainedBytes {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for RetainedBytes {}

impl From<Vec<u8>> for RetainedBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from_vec(bytes)
    }
}

impl From<Arc<Vec<u8>>> for RetainedBytes {
    fn from(bytes: Arc<Vec<u8>>) -> Self {
        Self::from_arc(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct StaticSource(&'static [u8]);

    impl RetainedByteSource for StaticSource {
        fn as_slice(&self) -> &[u8] {
            self.0
        }
    }

    #[test]
    fn slices_share_vec_owner_identity() {
        let owner = Arc::new((0u8..16).collect::<Vec<_>>());
        let first = RetainedBytes::from_arc_slice(Arc::clone(&owner), 2, 4).unwrap();
        let second = RetainedBytes::from_arc_slice(Arc::clone(&owner), 8, 4).unwrap();

        assert_eq!(first.as_slice(), &[2, 3, 4, 5]);
        assert_eq!(second.as_slice(), &[8, 9, 10, 11]);
        assert!(first.shares_owner(&second));
        assert_eq!(first.owner_offset(), 2);
        assert_eq!(second.owner_offset(), 8);
    }

    #[test]
    fn external_owner_slicing_preserves_bounds_and_identity() {
        let owner = Arc::new(StaticSource(b"abcdefghijklmnop")) as Arc<dyn RetainedByteSource>;
        let first = RetainedBytes::from_external_owner_slice(Arc::clone(&owner), 1, 3).unwrap();
        let second = RetainedBytes::from_external_owner_slice(Arc::clone(&owner), 4, 4).unwrap();

        assert_eq!(first.as_slice(), b"bcd");
        assert_eq!(second.as_slice(), b"efgh");
        assert!(first.shares_owner(&second));
        assert!(matches!(
            RetainedBytes::from_external_owner_slice(owner, 15, 2),
            Err(CoveError::OffsetRange)
        ));
    }
}
