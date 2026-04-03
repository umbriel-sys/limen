//! Memory classes, placement policy, and token-backed message storage.
//!
//! Edges carry [`MessageToken`](crate::types::MessageToken) handles rather than
//! full messages. All message data lives in a [`MemoryManager`](manager::MemoryManager),
//! keyed by token. This module provides:
//!
//! - [`MemoryClass`] — tag describing where a payload resides (host, pinned, device, shared).
//! - [`PlacementAcceptance`] / [`PlacementDecision`] — zero-copy routing policy.
//! - [`BufferDescriptor`] — lightweight byte-size descriptor returned by [`Payload`](crate::message::payload::Payload).
//!
//! Submodules:
//! - [`header_store`] — [`HeaderStore`](header_store::HeaderStore) supertrait for payload-agnostic header access.
//! - [`manager`] — [`MemoryManager`](manager::MemoryManager) typed storage interface.
//! - [`static_manager`] — `no_std`/`no_alloc` fixed-capacity implementation.
//! - [`heap_manager`] (`alloc`) — heap-backed fixed-capacity implementation.
//! - [`concurrent_manager`] (`std`) — lock-free freelist + per-slot `RwLock` implementation.

pub mod header_store;
pub mod manager;
pub mod static_manager;

#[cfg(feature = "alloc")]
pub mod heap_manager;

#[cfg(feature = "std")]
pub mod concurrent_manager;

/// The memory class associated with a payload.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryClass {
    /// Regular host memory.
    Host,
    /// Pinned (page-locked) host memory, suitable for DMA.
    PinnedHost,
    /// A device-specific memory region (e.g., GPU/NPU).
    Device(u8),
    /// A shared region accessible by multiple devices.
    Shared,
}

#[allow(clippy::derivable_impls)]
impl Default for MemoryClass {
    fn default() -> Self {
        MemoryClass::Host
    }
}

/* ------------------------ Bit layout (no magic numbers) ------------------------ */

const HOST_BIT: u32 = 0;
const PINNED_HOST_BIT: u32 = 1;
const DEVICE_BASE_BIT: u32 = 2;
const DEVICE_MAX_ORDINAL: u8 = 15; // supports Device(0..=15)
const SHARED_BIT: u32 = 18;

const fn device_bit(ordinal: u8) -> u32 {
    DEVICE_BASE_BIT + ordinal as u32
}

const DEVICE_MASK: u32 = {
    // 16 device bits starting at DEVICE_BASE_BIT
    ((1u32 << ((DEVICE_MAX_ORDINAL as u32) + 1)) - 1) << DEVICE_BASE_BIT
};

/* -------------------- Acceptance set (payload placement policy) -------------------- */

/// A bitfield describing which memory classes a port can accept zero-copy.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementAcceptance {
    bits: u32,
}

impl Default for PlacementAcceptance {
    fn default() -> Self {
        // default: accept host memory only
        Self::empty().with_host()
    }
}

impl PlacementAcceptance {
    /// Create an empty acceptance set.
    #[inline]
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    /// Construct from raw bits (advanced).
    #[inline]
    pub const fn from_bits(bits: u32) -> Self {
        Self { bits }
    }

    /// Return the raw bits.
    #[inline]
    pub const fn bits(&self) -> &u32 {
        &self.bits
    }

    /// Return true if there are no accepted classes.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.bits == 0
    }

    /// Union (set OR).
    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }

    /// Intersection (set AND).
    #[inline]
    pub const fn intersect(self, other: Self) -> Self {
        Self {
            bits: self.bits & other.bits,
        }
    }

    /// Superset test: does `self` include every class in `other`?
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.bits & other.bits) == other.bits
    }

    /// Create an acceptance set that includes all host classes.
    #[inline]
    pub const fn host_all() -> Self {
        Self::empty().with_host().with_pinned_host()
    }

    /// Accept regular host memory.
    #[inline]
    pub const fn with_host(mut self) -> Self {
        self.bits |= 1 << HOST_BIT;
        self
    }

    /// Accept pinned host memory.
    #[inline]
    pub const fn with_pinned_host(mut self) -> Self {
        self.bits |= 1 << PINNED_HOST_BIT;
        self
    }

    /// Accept a specific device ordinal (0..=15).
    #[inline]
    pub const fn with_device(mut self, ordinal: u8) -> Self {
        // Mask to supported range (keeps behavior compatible with your original code).
        let o = (ordinal & DEVICE_MAX_ORDINAL) as u32;
        self.bits |= 1 << (DEVICE_BASE_BIT + o);
        self
    }

    /// Like `with_device` but returns `None` if the ordinal is out of range.
    #[inline]
    pub const fn try_with_device(self, ordinal: u8) -> Option<Self> {
        if ordinal <= DEVICE_MAX_ORDINAL {
            Some(self.with_device(ordinal))
        } else {
            None
        }
    }

    /// Accept all supported device ordinals (0..=15).
    #[inline]
    pub const fn with_all_devices(mut self) -> Self {
        self.bits |= DEVICE_MASK;
        self
    }

    /// Accept shared memory regions.
    #[inline]
    pub const fn with_shared(mut self) -> Self {
        self.bits |= 1 << SHARED_BIT;
        self
    }

    /// Test whether the given memory class is accepted without copy.
    #[inline]
    pub const fn accepts(&self, class: MemoryClass) -> bool {
        match class {
            MemoryClass::Host => (self.bits & (1 << HOST_BIT)) != 0,
            MemoryClass::PinnedHost => (self.bits & (1 << PINNED_HOST_BIT)) != 0,
            MemoryClass::Device(o) => {
                let o = (o & DEVICE_MAX_ORDINAL) as u32;
                (self.bits & (1 << device_bit(o as u8))) != 0
            }
            MemoryClass::Shared => (self.bits & (1 << SHARED_BIT)) != 0,
        }
    }

    /// Convenience: build an acceptance set that accepts exactly one class.
    #[inline]
    pub const fn exactly(class: MemoryClass) -> Self {
        match class {
            MemoryClass::Host => Self::empty().with_host(),
            MemoryClass::PinnedHost => Self::empty().with_pinned_host(),
            MemoryClass::Device(o) => Self::empty().with_device(o),
            MemoryClass::Shared => Self::empty().with_shared(),
        }
    }
}

/// A descriptor of a buffer/payload view for size accounting.
///
/// Memory class information is now owned by the [`MemoryManager`] rather than
/// the payload itself — see `MemoryManager::memory_class()`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferDescriptor {
    /// The byte size of the payload.
    bytes: usize,
}

impl BufferDescriptor {
    /// Construct a new buffer descriptor.
    #[inline]
    pub const fn new(bytes: usize) -> Self {
        Self { bytes }
    }

    /// Return the byte size of the payload.
    #[inline]
    pub const fn bytes(&self) -> &usize {
        &self.bytes
    }
}

/// The edge-level placement decision for a message about to cross a port.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementDecision {
    /// Input port accepts the current placement; pass through zero-copy.
    ZeroCopy,
    /// Input port does not accept the current placement; adapt (copy/transfer) required.
    AdaptRequired,
}

/// Decide whether an edge can pass a message zero-copy or requires adaptation.
#[inline]
pub const fn decide_placement(
    acceptance: PlacementAcceptance,
    current: MemoryClass,
) -> PlacementDecision {
    if acceptance.accepts(current) {
        PlacementDecision::ZeroCopy
    } else {
        PlacementDecision::AdaptRequired
    }
}
