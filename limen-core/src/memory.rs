//! Memory classes and placement descriptors for zero-copy data paths.

/// The memory class associated with a payload.
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

impl Default for MemoryClass {
    fn default() -> Self {
        // default: regular host memory
        MemoryClass::Host
    }
}

/// A bitfield describing which memory classes a port can accept zero-copy.
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
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    /// Create an acceptance set that includes all host classes.
    pub const fn host_all() -> Self {
        let mut s = Self::empty();
        s = s.with_host();
        s = s.with_pinned_host();
        s
    }

    /// Accept regular host memory.
    pub const fn with_host(mut self) -> Self {
        self.bits |= 1 << 0;
        self
    }

    /// Accept pinned host memory.
    pub const fn with_pinned_host(mut self) -> Self {
        self.bits |= 1 << 1;
        self
    }

    /// Accept a specific device ordinal (0..=15).
    pub const fn with_device(mut self, ordinal: u8) -> Self {
        let o = (ordinal & 0x0F) as u32;
        self.bits |= 1 << (2 + o);
        self
    }

    /// Accept shared memory regions.
    pub const fn with_shared(mut self) -> Self {
        self.bits |= 1 << 18;
        self
    }

    /// Test whether the given memory class is accepted without copy.
    pub const fn accepts(&self, class: MemoryClass) -> bool {
        match class {
            MemoryClass::Host => (self.bits & (1 << 0)) != 0,
            MemoryClass::PinnedHost => (self.bits & (1 << 1)) != 0,
            MemoryClass::Device(o) => {
                let o = (o & 0x0F) as u32;
                (self.bits & (1 << (2 + o))) != 0
            }
            MemoryClass::Shared => (self.bits & (1 << 18)) != 0,
        }
    }
}

/// A descriptor of a buffer/payload view for size accounting and placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferDescriptor {
    /// The byte size of the payload.
    pub bytes: usize,
    /// The memory class where this payload currently resides.
    pub class: MemoryClass,
}
