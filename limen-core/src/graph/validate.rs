//! Descriptor validation helpers (no-alloc port checks, acyclicity with/without `alloc`).

use crate::errors::GraphError;
use crate::node::descriptor::NodeDescriptor;
use crate::node::NodeKind;
use crate::queue::descriptor::EdgeDescriptor;

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
    for e in edges {
        indeg[e.downstream.node.0] += 1;
    }

    let mut stack = Vec::with_capacity(n);
    for i in 0..n {
        if indeg[i] == 0 {
            stack.push(i);
        }
    }

    let mut visited = 0usize;
    while let Some(u) = stack.pop() {
        visited += 1;
        for e in edges.iter().filter(|e| e.upstream.node.0 == u) {
            let v = e.downstream.node.0;
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

/// Validate acyclicity without allocation using fixed-size arrays on the stack.
///
/// This variant is intended for [`GraphDescBuf`] where `N` is known at compile-time.
pub fn validate_acyclic_buf<const N: usize>(
    nodes: &[NodeDescriptor; N],
    edges: &[EdgeDescriptor],
) -> Result<(), GraphError> {
    // Simple Kahn's algorithm with stack-allocated arrays.
    let mut indeg = [0usize; N];
    for e in edges {
        let to = e.downstream.node.0;
        // Safety: descriptors are pre-validated, but guard anyway.
        if to >= N {
            return Err(GraphError::IncompatiblePorts);
        }
        indeg[to] += 1;
    }

    let mut stack = [0usize; N];
    let mut top = 0usize;

    // Seed with zero indegree nodes.
    for i in 0..N {
        if indeg[i] == 0 {
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
            let v = e.downstream.node.0;
            if v >= N {
                return Err(GraphError::IncompatiblePorts);
            }
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
