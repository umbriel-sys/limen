//! Node descriptor types.

use crate::{
    message::payload::Payload,
    node::{Node, NodeKind, NodePolicy},
    types::{NodeIndex, PortId, PortIndex},
};

/// A node descriptor: topology and policy metadata, without an executable instance.
///
/// `NodeDesc` captures static configuration of a node in the graph:
/// its identity, kind, port counts, policy, and an optional name.
/// It does not hold runtime state or implementation details.
#[derive(Debug, Clone)]
pub struct NodeDescriptor {
    /// Unique identifier of this node in the graph.
    pub id: NodeIndex,
    /// High-level category of the node (source, process, sink, etc).
    pub kind: NodeKind,
    /// Number of input ports declared by this node.
    pub in_ports: u16,
    /// Number of output ports declared by this node.
    pub out_ports: u16,
    /// Optional static name (for diagnostics or graph tooling).
    pub name: Option<&'static str>,
}

/// A lightweight descriptor that **links to** a concrete node instance and records its
/// static topology and policy metadata.
///
/// Unlike a standalone descriptor value, `NodeDescLink` borrows (`&'a`) the actual
/// node implementation (`N`). This makes the descriptor a *view* over a live node:
/// it does not own runtime state, but it exposes the node’s identity, kind, port
/// counts, scheduling policy, and optional name for graph construction, scheduling,
/// diagnostics, and tooling.
///
/// # Type Parameters
/// - `'a`: Lifetime of the borrowed node reference. The descriptor cannot outlive the node.
/// - `N`: Concrete node type implementing `Node<IN, OUT, InP, OutP>`.
/// - `IN`: Compile-time number of input ports for the node.
/// - `OUT`: Compile-time number of output ports for the node.
/// - `InP`: Input payload type (must implement `Payload`).
/// - `OutP`: Output payload type (must implement `Payload`).
///
/// # Invariants
/// Callers should ensure `in_ports == IN as u16` and `out_ports == OUT as u16` so the
/// stored counts are consistent with the node’s const-generic port arity.
#[derive(Debug, Clone)]
pub struct NodeLink<'a, N, const IN: usize, const OUT: usize, InP, OutP>
where
    InP: Payload,
    OutP: Payload,
    N: Node<IN, OUT, InP, OutP>,
{
    /// Borrowed handle to the concrete node instance.
    ///
    /// The descriptor does not own the node; the `'a` lifetime ties this reference
    /// to the node’s lifetime.
    node: &'a N,

    /// Unique identifier of this node within the graph.
    id: NodeIndex,

    /// Optional static name used for diagnostics or graph tooling.
    name: Option<&'static str>,

    /// Marker to bind `InP` and `OutP` into the type without storing values.
    ///
    /// This has zero runtime cost and exists solely for type tracking.
    _payload_marker: core::marker::PhantomData<(InP, OutP)>,
}

impl<'a, N, const IN: usize, const OUT: usize, InP, OutP> NodeLink<'a, N, IN, OUT, InP, OutP>
where
    InP: Payload,
    OutP: Payload,
    N: Node<IN, OUT, InP, OutP>,
{
    /// Construct a new `NodeLink` that borrows the given node and records its metadata.
    ///
    /// # Parameters
    /// - `node`: Borrowed reference to the concrete node instance.
    /// - `id`: Unique identifier of the node in the graph.
    /// - `name`: Optional static name for diagnostics or tooling.
    pub fn new(node: &'a N, id: NodeIndex, name: Option<&'static str>) -> Self {
        Self {
            node,
            id,
            name,
            _payload_marker: core::marker::PhantomData,
        }
    }

    /// Get a reference to the borrowed node.
    #[inline]
    pub fn node(&self) -> &'a N {
        self.node
    }

    /// Get the unique identifier of this node.
    #[inline]
    pub fn id(&self) -> NodeIndex {
        self.id
    }

    /// Returns the input port ids for the node.
    #[inline]
    pub fn input_port_ids(&self) -> [PortId; IN] {
        core::array::from_fn(|i| PortId {
            node: self.id,
            port: PortIndex(i),
        })
    }

    /// Returns the input port ids for the node.
    #[inline]
    pub fn output_port_ids(&self) -> [PortId; OUT] {
        core::array::from_fn(|i| PortId {
            node: self.id,
            port: PortIndex(i),
        })
    }

    /// Return the node's policy bundle.
    pub fn policy(&self) -> NodePolicy {
        self.node.policy()
    }

    /// Get the optional static name of this node.
    #[inline]
    pub fn name(&self) -> Option<&'static str> {
        self.name
    }

    /// Return the `NodeDescriptor` for this `NodeLink`.
    #[inline]
    pub fn descriptor(&self) -> NodeDescriptor {
        NodeDescriptor {
            id: self.id(),
            kind: self.node.node_kind(),
            in_ports: IN as u16,
            out_ports: OUT as u16,
            name: self.name(),
        }
    }
}
