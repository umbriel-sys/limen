//! Ergonomic builder that produces a validated, owned graph descriptor.
//!
//! The builder is `alloc`-only. It carries no node/queue instances—only
//! topology and policies. Use this in P1/P2 tooling and tests; P0 uses
//! `GraphDescBuf` or borrowed statics.

extern crate alloc;

use alloc::vec::Vec;

use crate::errors::GraphError;
use crate::node::descriptor::NodeDescriptor;
use crate::node::{NodeKind, NodePolicy};
use crate::policy::EdgePolicy;
use crate::queue::descriptor::EdgeDescriptor;
use crate::types::{EdgeIndex, NodeIndex, PortId, PortIndex};

use super::descriptor::{GraphDesc, GraphDescOwned, GraphValidator};
use super::validate::{validate_acyclic_alloc, validate_ports};

/// Opaque handle returned when adding a node; can emit in/out port handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeHandle {
    pub(crate) id: NodeIndex,
    in_ports: u16,
    out_ports: u16,
}

impl NodeHandle {
    #[inline]
    pub fn id(&self) -> NodeIndex {
        self.id
    }
    #[inline]
    pub fn in_port(&self, idx: u16) -> PortId {
        debug_assert!(idx < self.in_ports, "in_port out of range");
        PortId {
            node: self.id,
            port: PortIndex(idx as usize),
        }
    }
    #[inline]
    pub fn out_port(&self, idx: u16) -> PortId {
        debug_assert!(idx < self.out_ports, "out_port out of range");
        PortId {
            node: self.id,
            port: PortIndex(idx as usize),
        }
    }
}

/// Builder for graph descriptors (alloc).
pub struct GraphBuilder {
    nodes: Vec<NodeDescriptor>,
    edges: Vec<EdgeDescriptor>,

    // Port usage accounting for fast sanity checks.
    in_used: Vec<u16>,  // per-node: count of bound input ports
    out_used: Vec<u16>, // per-node: count of bound output ports
}

impl GraphBuilder {
    #[inline]
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            in_used: Vec::new(),
            out_used: Vec::new(),
        }
    }

    /// Add a node of a given `kind` with explicit port counts and node policy.
    pub fn add_node(
        &mut self,
        kind: NodeKind,
        in_ports: u16,
        out_ports: u16,
        name: Option<&'static str>,
    ) -> NodeHandle {
        let id = NodeIndex(self.nodes.len());
        self.nodes.push(NodeDescriptor {
            id,
            kind,
            in_ports,
            out_ports,
            name,
        });
        self.in_used.push(0);
        self.out_used.push(0);
        NodeHandle {
            id,
            in_ports,
            out_ports,
        }
    }

    #[inline]
    pub fn add_source(
        &mut self,
        out_ports: u16,
        policy: NodePolicy,
        name: Option<&'static str>,
    ) -> NodeHandle {
        self.add_node(NodeKind::Source, 0, out_ports, name)
    }
    #[inline]
    pub fn add_sink(
        &mut self,
        in_ports: u16,
        policy: NodePolicy,
        name: Option<&'static str>,
    ) -> NodeHandle {
        self.add_node(NodeKind::Sink, in_ports, 0, name)
    }
    #[inline]
    pub fn add_process(
        &mut self,
        in_ports: u16,
        out_ports: u16,
        policy: NodePolicy,
        name: Option<&'static str>,
    ) -> NodeHandle {
        self.add_node(NodeKind::Process, in_ports, out_ports, name)
    }
    #[inline]
    pub fn add_model(
        &mut self,
        in_ports: u16,
        out_ports: u16,
        policy: NodePolicy,
        name: Option<&'static str>,
    ) -> NodeHandle {
        self.add_node(NodeKind::Model, in_ports, out_ports, name)
    }
    #[inline]
    pub fn add_split(
        &mut self,
        in_ports: u16,
        out_ports: u16,
        policy: NodePolicy,
        name: Option<&'static str>,
    ) -> NodeHandle {
        debug_assert!(out_ports >= 2);
        self.add_node(NodeKind::Split, in_ports, out_ports, name)
    }
    #[inline]
    pub fn add_join(
        &mut self,
        in_ports: u16,
        out_ports: u16,
        policy: NodePolicy,
        name: Option<&'static str>,
    ) -> NodeHandle {
        debug_assert!(in_ports >= 2);
        self.add_node(NodeKind::Join, in_ports, out_ports, name)
    }
    #[inline]
    pub fn add_external(
        &mut self,
        in_ports: u16,
        out_ports: u16,
        policy: NodePolicy,
        name: Option<&'static str>,
    ) -> NodeHandle {
        self.add_node(NodeKind::External, in_ports, out_ports, name)
    }

    /// Connect `from.out_port(p)` → `to.in_port(q)` with an `EdgePolicy`.
    pub fn connect(&mut self, from: PortId, to: PortId, name: Option<&'static str>) -> EdgeIndex {
        // Bounds checks against declared port counts.
        let f = from.node.0;
        let t = to.node.0;

        assert!(f < self.nodes.len(), "from.node out of range");
        assert!(t < self.nodes.len(), "to.node out of range");

        assert!(
            from.port.0 < self.nodes[f].out_ports as usize,
            "from.port out of range"
        );
        assert!(
            to.port.0 < self.nodes[t].in_ports as usize,
            "to.port out of range"
        );

        // Coarse per-node occupancy sanity (uniqueness per input is enforced in final validation).
        let used_in = &mut self.in_used[t];
        assert!(
            *used_in <= self.nodes[t].in_ports,
            "too many inputs bound for target node"
        );
        *used_in += 1;

        let used_out = &mut self.out_used[f];
        assert!(
            *used_out <= self.nodes[f].out_ports,
            "too many outputs bound for source node"
        );
        *used_out += 1;

        let id = EdgeIndex(self.edges.len());
        let ed = EdgeDescriptor {
            id,
            upstream: PortId {
                node: from.node,
                port: from.port,
            },

            downstream: PortId {
                node: to.node,
                port: to.port,
            },
            name,
        };
        self.edges.push(ed);
        EdgeIndex(self.edges.len() - 1)
    }

    /// Finish the builder and return a validated owned descriptor.
    pub fn build(self) -> Result<GraphDescOwned, GraphError> {
        // Basic port usage sanity.
        validate_ports(&self.nodes, &self.edges)?;
        // Acyclicity (DAG).
        validate_acyclic_alloc(&self.nodes, &self.edges)?;
        Ok(GraphDescOwned {
            nodes: self.nodes,
            edges: self.edges,
        })
    }
}
