//! Graph validation interface.

use crate::edge::link::EdgeDescriptor;
use crate::errors::GraphError;
use crate::node::link::NodeDescriptor;
use crate::node::source::EXTERNAL_INGRESS_NODE;
use crate::node::NodeKind;

// no_std + alloc: bring in the `vec!` macro only
#[cfg(all(feature = "alloc", not(feature = "std")))]
use alloc::vec;

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

/// Validates graph ports, including any number of synthetic ingress "monitor edges".
///
/// A monitor edge is an `EdgeDescriptor` whose `upstream.node == EXTERNAL_INGRESS_NODE`.
/// It *does not* consume a real input port on the downstream node and is only valid
/// when the downstream node `kind == Source`. At most **one** monitor edge may target
/// each individual Source node.
pub fn validate_ports(
    nodes: &[NodeDescriptor],
    edges: &[EdgeDescriptor],
) -> Result<(), GraphError> {
    let n = nodes.len();

    // (A) Node id ↔ index must match exactly (0..N) and kind/arity constraints.
    for (i, nd) in nodes.iter().enumerate() {
        if nd.id.as_usize() != &i {
            return Err(GraphError::IncompatiblePorts);
        }
        match nd.kind {
            NodeKind::Source => {
                if nd.in_ports != 0 || nd.out_ports < 1 {
                    return Err(GraphError::IncompatiblePorts);
                }
            }
            NodeKind::Sink => {
                if nd.in_ports < 1 || nd.out_ports != 0 {
                    return Err(GraphError::IncompatiblePorts);
                }
            }
            NodeKind::Split => {
                if nd.in_ports < 1 || nd.out_ports < 2 {
                    return Err(GraphError::IncompatiblePorts);
                }
            }
            NodeKind::Join => {
                if nd.in_ports < 2 || nd.out_ports < 1 {
                    return Err(GraphError::IncompatiblePorts);
                }
            }
            NodeKind::Process | NodeKind::Model => {
                if nd.in_ports < 1 || nd.out_ports < 1 {
                    return Err(GraphError::IncompatiblePorts);
                }
            }
            NodeKind::External => {
                // No fixed arity constraints here.
            }
        }
    }

    // (B) Edge ids, endpoints, and port bounds.
    // Monitor edges (from EXTERNAL_INGRESS_NODE) are allowed to target any Source.
    // They do not consume a real port, so we bypass downstream port checks for them.
    for (i, ed) in edges.iter().enumerate() {
        // strict id match
        if ed.id().as_usize() != &i {
            return Err(GraphError::IncompatiblePorts);
        }

        let is_monitor = ed.upstream().node() == &EXTERNAL_INGRESS_NODE;
        let t = ed.downstream().node().as_usize();

        if is_monitor {
            // Downstream must be real and a Source.
            if t >= &n {
                return Err(GraphError::IncompatiblePorts);
            }
            if nodes[*t].kind != NodeKind::Source {
                return Err(GraphError::IncompatiblePorts);
            }

            // Enforce "at most one monitor edge per Source node" via nested scan.
            for (j, other) in edges.iter().enumerate() {
                if j == i {
                    continue;
                }
                if other.upstream().node() == &EXTERNAL_INGRESS_NODE
                    && other.downstream().node().as_usize() == t
                {
                    return Err(GraphError::IncompatiblePorts);
                }
            }

            // No port-bound checks for monitor edges; they don't consume real ports.
            continue;
        }

        // Regular edge: both endpoints must be real nodes in range.
        let f = ed.upstream().node().as_usize();
        if f >= &n || t >= &n {
            return Err(GraphError::IncompatiblePorts);
        }

        let nf = &nodes[*f];
        let nt = &nodes[*t];

        // Regular port bounds.
        if *ed.upstream().port().as_usize() >= nf.out_ports as usize {
            return Err(GraphError::IncompatiblePorts);
        }
        if *ed.downstream().port().as_usize() >= nt.in_ports as usize {
            return Err(GraphError::IncompatiblePorts);
        }
    }

    // (C) Each (to_node, to_port) must be unique among REAL edges.
    // Monitor edges are excluded because they do not occupy a real input port.
    for (i, ei) in edges.iter().enumerate() {
        if ei.upstream().node() == &EXTERNAL_INGRESS_NODE {
            continue; // skip monitors
        }
        for ej in edges.iter().skip(i + 1) {
            if ej.upstream().node() == &EXTERNAL_INGRESS_NODE {
                continue; // skip monitors
            }
            if ei.downstream().node().as_usize() == ej.downstream().node().as_usize()
                && ei.downstream().port().as_usize() == ej.downstream().port().as_usize()
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
///
/// Monitor edges are ignored for cycle detection (they have no real upstream node
/// and cannot introduce a cycle).
pub fn validate_acyclic_buf<const N: usize>(
    _nodes: &[NodeDescriptor; N],
    edges: &[EdgeDescriptor],
) -> Result<(), GraphError> {
    // Simple Kahn's algorithm with stack-allocated arrays.
    let mut indeg = [0usize; N];

    // Validate both ends and build indegree.
    for e in edges {
        if e.upstream().node() == &EXTERNAL_INGRESS_NODE {
            continue; // ignore monitor edge
        }
        let u = e.upstream().node().as_usize();
        let v = e.downstream().node().as_usize();
        if u >= &N || v >= &N {
            return Err(GraphError::IncompatiblePorts);
        }
        indeg[*v] += 1;
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

        // Decrement indegree of real outgoing edges from u.
        for e in edges.iter().filter(|e| {
            e.upstream().node() != &EXTERNAL_INGRESS_NODE && e.upstream().node().as_usize() == &u
        }) {
            let v = e.downstream().node().as_usize();
            debug_assert!(v < &N, "downstream index validated earlier");
            indeg[*v] -= 1;
            if indeg[*v] == 0 {
                stack[top] = *v;
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
    use alloc::vec::Vec;

    let n = nodes.len();
    let mut indeg = vec![0usize; n];

    // Validate indices and build indegree for real edges only.
    for e in edges {
        if e.upstream().node() == &EXTERNAL_INGRESS_NODE {
            continue;
        }
        let u = e.upstream().node().as_usize();
        let v = e.downstream().node().as_usize();
        if u >= &n || v >= &n {
            return Err(GraphError::IncompatiblePorts);
        }
        indeg[*v] += 1;
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
        for e in edges.iter().filter(|e| {
            e.upstream().node() != &EXTERNAL_INGRESS_NODE && e.upstream().node().as_usize() == &u
        }) {
            let v = e.downstream().node().as_usize();
            indeg[*v] -= 1;
            if indeg[*v] == 0 {
                stack.push(*v);
            }
        }
    }

    if visited != n {
        return Err(GraphError::Cyclic);
    }
    Ok(())
}
