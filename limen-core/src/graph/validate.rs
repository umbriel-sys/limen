//! Graph validation interface.

use crate::errors::GraphError;
use crate::node::link::NodeDescriptor;
use crate::node::NodeKind;
use crate::queue::link::EdgeDescriptor;

#[cfg(not(feature = "std"))]
extern crate alloc;
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};

/// An interface for descriptor validation (borrowed/owned/buffer).
pub trait GraphValidator {
    /// Validates the wiring of the graph.
    fn validate(&self) -> Result<(), GraphError>;
}

/// Owned, no-alloc descriptor (arrays are stored by value).
#[derive(Debug, Clone)]
pub struct GraphDescBuf<const N: usize, const E: usize> {
    /// Nodes descriptors.
    pub nodes: [NodeDescriptor; N],
    /// Edge descriptors.
    pub edges: [EdgeDescriptor; E],
}

impl<const N: usize, const E: usize> GraphValidator for GraphDescBuf<N, E> {
    #[inline]
    fn validate(&self) -> Result<(), GraphError> {
        validate_ports(&self.nodes, &self.edges)?;
        #[cfg(not(feature = "alloc"))]
        {
            validate_acyclic_buf::<N>(&self.nodes, &self.edges)?;
        }
        #[cfg(feature = "alloc")]
        {
            validate_acyclic_alloc(&self.nodes, &self.edges)?;
        }

        Ok(())
    }
}

/// Validate port bounds and uniqueness (no allocation).
pub fn validate_ports(
    nodes: &[NodeDescriptor],
    edges: &[EdgeDescriptor],
) -> Result<(), GraphError> {
    let n = nodes.len();

    // Validate kind in/out constraints.
    for nd in nodes {
        match nd.kind {
            NodeKind::Source => {
                if nd.in_ports != 0 {
                    return Err(GraphError::IncompatiblePorts);
                }
            }
            NodeKind::Sink => {
                if nd.out_ports != 0 {
                    return Err(GraphError::IncompatiblePorts);
                }
            }
            NodeKind::Split => {
                if nd.out_ports < 2 {
                    return Err(GraphError::IncompatiblePorts);
                }
            }
            NodeKind::Join => {
                if nd.in_ports < 2 {
                    return Err(GraphError::IncompatiblePorts);
                }
            }
            NodeKind::Process | NodeKind::Model | NodeKind::External => {}
        }
    }

    // Bounds for each edge.
    for e in edges {
        let f = e.upstream.node.0;
        let t = e.downstream.node.0;
        if f >= n || t >= n {
            return Err(GraphError::IncompatiblePorts);
        }
        let nf = &nodes[f];
        let nt = &nodes[t];
        if e.upstream.port.0 >= nf.out_ports as usize {
            return Err(GraphError::IncompatiblePorts);
        }
        if e.downstream.port.0 >= nt.in_ports as usize {
            return Err(GraphError::IncompatiblePorts);
        }
    }

    // Each (to, to_port) must be unique.
    for (i, ei) in edges.iter().enumerate() {
        for ej in edges.iter().skip(i + 1) {
            if ei.downstream.node.0 == ej.downstream.node.0
                && ei.downstream.port.0 == ej.downstream.port.0
            {
                return Err(GraphError::IncompatiblePorts);
            }
        }
    }

    Ok(())
}

/// Validate acyclicity without allocation using fixed-size arrays on the stack.
///
/// This variant is intended for [`GraphDescBuf`] where `N` is known at compile-time.
pub fn validate_acyclic_buf<const N: usize>(
    _nodes: &[NodeDescriptor; N],
    edges: &[EdgeDescriptor],
) -> Result<(), GraphError> {
    // Simple Kahn's algorithm with stack-allocated arrays.
    let mut indeg = [0usize; N];

    // Validate both ends and build indegree.
    for e in edges {
        let u = e.upstream.node.0;
        let v = e.downstream.node.0;
        if u >= N || v >= N {
            return Err(GraphError::IncompatiblePorts);
        }
        indeg[v] += 1;
    }

    let mut stack = [0usize; N];
    let mut top = 0usize;

    // Seed with zero indegree nodes.
    for (i, &deg) in indeg.iter().enumerate() {
        if deg == 0 {
            stack[top] = i;
            top += 1;
        }
    }

    let mut visited = 0usize;
    while top > 0 {
        top -= 1;
        let u = stack[top];
        visited += 1;

        for e in edges.iter().filter(|e| e.upstream.node.0 == u) {
            let v = e.downstream.node.0; // v ∈ [0, N) due to earlier validation
            debug_assert!(v < N, "downstream index validated earlier");
            indeg[v] -= 1;
            if indeg[v] == 0 {
                stack[top] = v;
                top += 1;
            }
        }
    }

    if visited != N {
        return Err(GraphError::Cyclic);
    }
    Ok(())
}

/// Validate acyclicity using Kahn's algorithm (requires `alloc`).
#[cfg(feature = "alloc")]
pub fn validate_acyclic_alloc(
    nodes: &[NodeDescriptor],
    edges: &[EdgeDescriptor],
) -> Result<(), GraphError> {
    extern crate alloc;
    use alloc::vec::Vec;

    let n = nodes.len();
    let mut indeg = vec![0usize; n];

    // Validate indices and build indegree.
    for e in edges {
        let u = e.upstream.node.0;
        let v = e.downstream.node.0;
        if u >= n || v >= n {
            return Err(GraphError::IncompatiblePorts);
        }
        indeg[v] += 1;
    }

    // Seed stack with zero-indegree nodes.
    let mut stack = Vec::with_capacity(n);
    for (i, &deg) in indeg.iter().take(n).enumerate() {
        if deg == 0 {
            stack.push(i);
        }
    }

    // Kahn's main loop.
    let mut visited = 0usize;
    while let Some(u) = stack.pop() {
        visited += 1;
        for e in edges.iter().filter(|e| e.upstream.node.0 == u) {
            let v = e.downstream.node.0; // v ∈ [0, n) due to earlier validation
            indeg[v] -= 1;
            if indeg[v] == 0 {
                stack.push(v);
            }
        }
    }

    if visited != n {
        return Err(GraphError::Cyclic);
    }
    Ok(())
}
