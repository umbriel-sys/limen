//! Descriptor types: node/edge records, borrowed/owned/buffer descriptors,
//! and a view/validation interface.

use crate::errors::GraphError;
use crate::memory::PlacementAcceptance;
use crate::node::descriptor::NodeDescriptor;
use crate::queue::descriptor::EdgeDescriptor;

use super::validate;

/// A lightweight, borrow-based graph descriptor (works in `no_std` via static slices).
#[derive(Debug, Clone, Copy)]
pub struct GraphDesc<'a> {
    pub nodes: &'a [NodeDescriptor],
    pub edges: &'a [EdgeDescriptor],
}

impl<'a> GraphDesc<'a> {
    /// Validate port bounds/uniqueness and acyclicity.
    ///
    /// In `no_alloc` builds, acyclicity is only fully validated for fixed-size
    /// buffers via [`GraphDescBuf`]. For borrowed slices (`GraphDesc`), this
    /// method performs port checks and (when `alloc`) a full DAG check.
    pub fn validate(&self) -> Result<(), GraphError> {
        validate::validate_ports(self.nodes, self.edges)?;

        // Acyclicity: if `alloc`, run Kahn's algorithm; otherwise skip here.
        #[cfg(feature = "alloc")]
        {
            validate::validate_acyclic_alloc(self.nodes, self.edges)?;
        }
        Ok(())
    }
}

/// An interface for descriptor validation (borrowed/owned/buffer).
pub trait GraphValidator {
    fn validate(&self) -> Result<(), GraphError>;
}

impl<'a> GraphValidator for GraphDesc<'a> {
    #[inline]
    fn validate(&self) -> Result<(), GraphError> {
        self.validate()
    }
}

/// Owned, no-alloc descriptor (arrays are stored by value).
#[derive(Debug, Clone)]
pub struct GraphDescBuf<const N: usize, const E: usize> {
    pub nodes: [NodeDescriptor; N],
    pub edges: [EdgeDescriptor; E],
}

impl<const N: usize, const E: usize> GraphDescBuf<N, E> {
    #[inline]
    pub fn as_borrowed(&self) -> GraphDesc<'_> {
        GraphDesc {
            nodes: &self.nodes,
            edges: &self.edges,
        }
    }
}

impl<const N: usize, const E: usize> GraphValidator for GraphDescBuf<N, E> {
    #[inline]
    fn validate(&self) -> Result<(), GraphError> {
        // Port checks never allocate.
        validate::validate_ports(&self.nodes, &self.edges)?;
        // Acyclicity (no-alloc) using fixed-size arrays on the stack.
        validate::validate_acyclic_buf::<N>(&self.nodes, &self.edges)?;
        Ok(())
    }
}

/// A read-only view over any graph descriptor storage.
pub trait GraphDescriptorView {
    fn nodes(&self) -> &[NodeDescriptor];
    fn edges(&self) -> &[EdgeDescriptor];

    #[inline]
    fn node_count(&self) -> usize {
        self.nodes().len()
    }
    #[inline]
    fn edge_count(&self) -> usize {
        self.edges().len()
    }
}

// Borrowed slices
impl<'a> GraphDescriptorView for GraphDesc<'a> {
    #[inline]
    fn nodes(&self) -> &[NodeDescriptor] {
        self.nodes
    }
    #[inline]
    fn edges(&self) -> &[EdgeDescriptor] {
        self.edges
    }
}

// Owned, no-alloc fixed-size buffer
impl<const N: usize, const E: usize> GraphDescriptorView for GraphDescBuf<N, E> {
    #[inline]
    fn nodes(&self) -> &[NodeDescriptor] {
        &self.nodes
    }
    #[inline]
    fn edges(&self) -> &[EdgeDescriptor] {
        &self.edges
    }
}

#[cfg(feature = "alloc")]
mod view_impl_owned {
    extern crate alloc;

    use super::*;
    use alloc::vec::Vec;

    /// Owned descriptor produced by the builder (impl of the view).
    #[derive(Debug, Clone)]
    pub struct GraphDescOwned {
        pub nodes: Vec<NodeDescriptor>,
        pub edges: Vec<EdgeDescriptor>,
    }

    impl GraphDescOwned {
        #[inline]
        pub fn as_borrowed(&self) -> GraphDesc<'_> {
            GraphDesc {
                nodes: &self.nodes,
                edges: &self.edges,
            }
        }
    }

    impl GraphValidator for GraphDescOwned {
        #[inline]
        fn validate(&self) -> Result<(), GraphError> {
            self.as_borrowed().validate()
        }
    }

    impl GraphDescriptorView for GraphDescOwned {
        #[inline]
        fn nodes(&self) -> &[NodeDescriptor] {
            &self.nodes
        }
        #[inline]
        fn edges(&self) -> &[EdgeDescriptor] {
            &self.edges
        }
    }

    pub use GraphDescOwned as _GraphDescOwnedExport;
}

// Re-export the alloc-owned type for crate users, if alloc enabled.
#[cfg(feature = "alloc")]
pub use view_impl_owned::_GraphDescOwnedExport as GraphDescOwned;
