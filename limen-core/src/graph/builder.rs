//! Holds Graph Builder TODO: update comment.

/// Build a fully-typed, zero-alloc graph struct and its trait impls.
///
/// This macro generates:
/// - A concrete graph struct holding `NodeLink` and `EdgeLink` tuples.
/// - `GraphApi<NODE_COUNT, EDGE_COUNT>` for descriptors and helpers.
/// - `GraphNodeAccess<I>` / `GraphEdgeAccess<E>` for indexed access.
/// - `GraphNodeTypes<I, IN, OUT>` and `GraphNodeContextBuilder<I, IN, OUT>`
///   per node, enabling compile-time-typed `StepContext` construction.
///
/// # When to use
/// Use `define_graph!` when you want to **describe** a static DAG of nodes and
/// queues once, and have compile-time wiring and types drive all runtime logic
/// (no trait impls per graph, no dynamic dispatch, `no_std`-friendly).
///
/// # Syntax
/// ```text
/// define_graph! {
///     [attrs] vis struct GraphName;
///
///     nodes {
///         idx: {
///             ty: NodeType,
///             in_ports: N_IN,             // usize literal
///             out_ports: N_OUT,           // usize literal
///             in_payload: InPayloadTy,    // implements Payload
///             out_payload: OutPayloadTy,  // implements Payload
///             name: Option<&'static str>  // label for diagnostics
///         },
///         ...
///     }
///
///     edges {
///         idx: {
///             ty: QueueType,              // implements SpscQueue<Item = Message<payload>>
///             payload: PayloadTy,         // message payload carried on this edge
///             from: (NODE_IDX, OUT_PORT), // producer node/port
///             to:   (NODE_IDX, IN_PORT),  // consumer node/port
///             policy: EdgePolicy,         // admission/backpressure
///             name: Option<&'static str>
///         },
///         ...
///     }
///
///     wiring {
///         node NODE_IDX: {
///             in:  [ EDGE_IDX, ... ],     // incoming edges in port order (len = N_IN)
///             out: [ EDGE_IDX, ... ]      // outgoing edges in port order (len = N_OUT)
///         },
///         ...
///     }
/// }
/// ```
///
/// # Requirements
/// - Each `NodeType` must be wrapped by `NodeLink<NodeType, IN, OUT, InP, OutP>`.
/// - Each `QueueType` must implement `SpscQueue<Item = Message<PayloadTy>>`.
/// - `payload` in `edges` must match the producing node’s `out_payload` and the
///   consuming node’s `in_payload`.
/// - `wiring` must list **exactly** `in_ports` incoming and `out_ports` outgoing
///   edges for each node, in **port index order**.
/// - `EdgePolicy` must be `Copy` (required by `make_step_context`).
///
/// # What gets generated
/// - `struct GraphName { nodes: (...), edges: (...) }`
/// - `impl GraphApi<N, M> for GraphName`
/// - `impl GraphNodeAccess<{I}> for GraphName` (for every node `I`)
/// - `impl GraphEdgeAccess<{E}> for GraphName` (for every edge `E`)
/// - `impl GraphNodeTypes<{I}, {IN}, {OUT}> for GraphName` (per node)
/// - `impl GraphNodeContextBuilder<{I}, {IN}, {OUT}> for GraphName` (per node)
///
/// # Example
/// ```rust
/// // Queue carrying Message<u32>.
/// pub struct QueueU32;
/// impl SpscQueue for QueueU32 {
///     type Item = Message<u32>;
///     fn try_push(&mut self, _i: Self::Item, _p: &EdgePolicy) -> EnqueueResult { EnqueueResult::Rejected }
///     fn try_pop(&mut self) -> Result<Self::Item, QueueError> { Err(QueueError::Empty) }
///     fn occupancy(&self, _p: &EdgePolicy) -> QueueOccupancy {
///         QueueOccupancy { items: 0, bytes: 0, watermark: WatermarkState::AtOrAboveHard }
///     }
///     fn try_peek(&self) -> Result<&Self::Item, QueueError> { Err(QueueError::Empty) }
/// }
///
/// // Minimal nodes (replace with real implementations).
/// pub struct SourceNode; pub struct MapNode; pub struct SinkNode;
/// impl Node<0,1,(),u32> for SourceNode { /* ... */ }
/// impl Node<1,1,u32,u32> for MapNode   { /* ... */ }
/// impl Node<1,0,u32,()> for SinkNode   { /* ... */ }
///
/// define_graph! {
///     /// A 3-stage pipeline: Source -> Map -> Sink
///     pub struct Pipeline;
///
///     nodes {
///         0: { ty: SourceNode, in_ports: 0, out_ports: 1, in_payload: (),  out_payload: u32, name: Some("source") },
///         1: { ty: MapNode,    in_ports: 1, out_ports: 1, in_payload: u32, out_payload: u32, name: Some("map")    },
///         2: { ty: SinkNode,   in_ports: 1, out_ports: 0, in_payload: u32, out_payload: (),  name: Some("sink")   }
///     }
///
///     edges {
///         0: { ty: QueueU32, payload: u32, from: (0,0), to: (1,0), policy: EdgePolicy::default(), name: Some("e0") },
///         1: { ty: QueueU32, payload: u32, from: (1,0), to: (2,0), policy: EdgePolicy::default(), name: Some("e1") }
///     }
///
///     wiring {
///         node 0: { in: [ ],   out: [ 0 ] },
///         node 1: { in: [ 0 ], out: [ 1 ] },
///         node 2: { in: [ 1 ], out: [ ] }
///     }
/// }
///
/// // Using the generated API:
/// fn run_one_round<C, T>(clock: &C, telemetry: &mut T) {
///     let mut g = Pipeline::new(SourceNode, MapNode, SinkNode, QueueU32, QueueU32);
///
///     // Static descriptors (fixed-size arrays).
///     let _nds = <Pipeline as GraphApi<3, 2>>::get_node_descriptors(&g);
///     let _eds = <Pipeline as GraphApi<3, 2>>::get_edge_descriptors(&g);
///
///     // Build a StepContext for node #1 (MapNode). IN=1, OUT=1 at the type level.
///     let mut ctx = <Pipeline as GraphApi<3, 2>>::make_step_context_for_node::<1, 1, 1, _, _>(
///         &mut g, clock, telemetry);
///
///     // Access the typed node handle and call step.
///     let link = <Pipeline as GraphApi<3, 2>>::get_node_mut::<1>(&mut g);
///     let node = link.node_mut();
///     type InQ  = <Pipeline as GraphNodeTypes<1, 1, 1>>::InQ;
///     type OutQ = <Pipeline as GraphNodeTypes<1, 1, 1>>::OutQ;
///     let _ = node.step::<InQ, OutQ, _, _>(&mut ctx);
/// }
/// ```
#[macro_export]
macro_rules! define_graph {
    // Initial arm, builds edge map for internal use.
    (
        $(#[$meta:meta])*
        $vis:vis struct $Graph:ident;

        nodes {
            $(
                $nidx:tt : {
                    ty: $nty:ty,
                    in_ports: $nin:expr,
                    out_ports: $nout:expr,
                    in_payload: $in_p:ty,
                    out_payload: $out_p:ty,
                    name: $nlabel:expr
                }
            ),+ $(,)?
        }

        edges {
            $(
                $eidx:tt : {
                    ty: $qty:ty,
                    payload: $ep:ty,
                    from: ($from_node:tt, $from_port:expr),
                    to:   ($to_node:tt,   $to_port:expr),
                    policy: $epol:expr,
                    name: $elabel:expr
                }
            ),+ $(,)?
        }

        wiring {
            $(
                node $w_node:tt : {
                    in:  [ $( $win:tt ),* $(,)? ],
                    out: [ $( $wout:tt ),* $(,)? ]
                }
            ),+ $(,)?
        }
    ) => {
        $crate::define_graph!(
            @emit_impls
            $(#[$meta])*
            $vis struct $Graph;

            nodes {
                $( $nidx : {
                    ty: $nty,
                    in_ports: $nin,
                    out_ports: $nout,
                    in_payload: $in_p,
                    out_payload: $out_p,
                    name: $nlabel
                } ),+
            }

            edges {
                $( $eidx : {
                    ty: $qty,
                    payload: $ep,
                    from: ($from_node, $from_port),
                    to:   ($to_node,   $to_port),
                    policy: $epol,
                    name: $elabel
                } ),+
            }

            wiring {
                $( node $w_node : {
                    in:  [ $( $win ),* ],
                    out: [ $( $wout ),* ]
                } ),+
            }

            // Single token-tree for node id → full node spec
            node_map [ $( $nidx : {
                ty: $nty,
                in_ports: $nin,
                out_ports: $nout,
                in_payload: $in_p,
               out_payload: $out_p,
                name: $nlabel
            } ),* ]

            // Single token-tree for edge id → queue type
            edge_map [ $( $eidx => $qty ),* ]
        );
    };

    // Main arm, builds structs and implements traits.
    (
        @emit_impls
        $(#[$meta:meta])*
        $vis:vis struct $Graph:ident;

        nodes {
            $(
                $nidx:tt : {
                    ty: $nty:ty,
                    in_ports: $nin:expr,
                    out_ports: $nout:expr,
                    in_payload: $in_p:ty,
                    out_payload: $out_p:ty,
                    name: $nlabel:expr
                }
            ),+ $(,)?
        }

        edges {
            $(
                $eidx:tt : {
                    ty: $qty:ty,
                    payload: $ep:ty,
                    from: ($from_node:tt, $from_port:expr),
                    to:   ($to_node:tt,   $to_port:expr),
                    policy: $epol:expr,
                    name: $elabel:expr
                }
            ),+ $(,)?
        }

        wiring {
            $(
                node $w_node:tt : {
                    in:  [ $( $win:tt ),* $(,)? ],
                    out: [ $( $wout:tt ),* $(,)? ]
                }
            ),+ $(,)?
        }

        node_map $NODE_MAP:tt

        edge_map $EDGE_MAP:tt
    ) => {
        $(#[$meta])*
        $vis struct $Graph {
            nodes: (
                $(
                    $crate::prelude::NodeLink<$nty, { $nin }, { $nout }, $in_p, $out_p>
                ),*
            ),
            edges: (
                $(
                    $crate::prelude::EdgeLink<$qty, $ep>
                ),*
            ),
        }

        impl $Graph {
            #[inline]
            pub fn new(
                $( ::paste::paste! { [<node_ $nidx>] }: $nty ),*,
                $( ::paste::paste! { [<q_ $eidx>] }: $qty ),*
            ) -> Self {
                let nodes = (
                    $(
                        $crate::prelude::NodeLink::<$nty, { $nin }, { $nout }, $in_p, $out_p>::new(
                            ::paste::paste! { [<node_ $nidx>] },
                            <$crate::prelude::NodeIndex as core::convert::From<usize>>::from($nidx),
                            $nlabel
                        )
                    ),*
                );
                let edges = (
                    $(
                        $crate::prelude::EdgeLink::<$qty, $ep>::new(
                            ::paste::paste! { [<q_ $eidx>] },
                            <$crate::prelude::EdgeIndex as core::convert::From<usize>>::from($eidx),
                            $crate::prelude::PortId {
                                node: <$crate::prelude::NodeIndex as core::convert::From<usize>>::from($from_node),
                                port: $crate::prelude::PortIndex($from_port),
                            },
                            $crate::prelude::PortId {
                                node: <$crate::prelude::NodeIndex as core::convert::From<usize>>::from($to_node),
                                port: $crate::prelude::PortIndex($to_port),
                            },
                            $epol,
                            $elabel
                        )
                    ),*
                );
                Self { nodes, edges }
            }
        }

        // GraphApi
        impl $crate::prelude::GraphApi<
            { $crate::define_graph!(@count $( $nidx )*) },
            { $crate::define_graph!(@count $( $eidx )*) }
        > for $Graph
        {
            #[inline]
            fn get_node_descriptors(&self) -> [$crate::prelude::NodeDescriptor; $crate::define_graph!(@count $( $nidx )*)] {
                [ $( self.nodes.$nidx.descriptor() ),* ]
            }
            #[inline]
            fn get_edge_descriptors(&self) -> [$crate::prelude::EdgeDescriptor; $crate::define_graph!(@count $( $eidx )*)] {
                [ $( self.edges.$eidx.descriptor() ),* ]
            }
        }

        // Per-node typed access
        $(
            impl $crate::prelude::GraphNodeAccess<{ $nidx }> for $Graph {
                type Node = $crate::prelude::NodeLink<$nty, { $nin }, { $nout }, $in_p, $out_p>;
                #[inline] fn node_ref(&self) -> &Self::Node { &self.nodes.$nidx }
                #[inline] fn node_mut(&mut self) -> &mut Self::Node { &mut self.nodes.$nidx }
            }
        )*

        // Per-edge typed access
        $(
            impl $crate::prelude::GraphEdgeAccess<{ $eidx }> for $Graph {
                type Edge = $crate::prelude::EdgeLink<$qty, $ep>;
                #[inline] fn edge_ref(&self) -> &Self::Edge { &self.edges.$eidx }
                #[inline] fn edge_mut(&mut self) -> &mut Self::Edge { &mut self.edges.$eidx }
            }
        )*

        // ---------- Per-node compile-time types/arity (const generics) ----------
        $(
            impl $crate::prelude::GraphNodeTypes<
            { $w_node },
            { $crate::define_graph!(@node_field in_ports  $w_node ;  $NODE_MAP  ) },
            { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
        > for $Graph
            {
                type InP  = $crate::define_graph!(@node_field in_payload  $w_node ; $NODE_MAP );
                type OutP = $crate::define_graph!(@node_field out_payload $w_node ; $NODE_MAP );

                type InQ  = $crate::define_graph!(@in_q_ty
                    $crate::define_graph!(@node_field in_payload  $w_node ; $NODE_MAP )
                    ;
                    [ $( $win ),* ] ;
                    $EDGE_MAP
                );

                type OutQ = $crate::define_graph!(@out_q_ty
                    $crate::define_graph!(@node_field out_payload $w_node ; $NODE_MAP )
                    ;
                    [ $( $wout ),* ] ;
                    $EDGE_MAP
                );
            }
        )*

        // ---------- Per-node StepContext builder ----------
$(
    impl $crate::prelude::GraphNodeContextBuilder<
        { $w_node },
        { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
        { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
    > for $Graph
    {
        #[inline]
        fn make_step_context<C, T>(
            &mut self,
            clock: &C,
            telemetry: &mut T,
        ) -> $crate::prelude::StepContext<
            { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
            { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) },
            <Self as $crate::prelude::GraphNodeTypes<
                { $w_node },
                { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
                { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
            >>::InP,
            <Self as $crate::prelude::GraphNodeTypes<
                { $w_node },
                { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
                { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
            >>::OutP,
            <Self as $crate::prelude::GraphNodeTypes<
                { $w_node },
                { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
                { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
            >>::InQ,
            <Self as $crate::prelude::GraphNodeTypes<
                { $w_node },
                { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
                { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
            >>::OutQ,
            C, T
        >
        where
            $crate::prelude::EdgePolicy: Copy,
        {
            let inputs: [&mut <Self as $crate::prelude::GraphNodeTypes<
                { $w_node },
                { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
                { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
            >>::InQ;
                { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) }
            ] = [
                $( self.edges.$win.queue_mut() ),*
            ];

            let outputs: [&mut <Self as $crate::prelude::GraphNodeTypes<
                { $w_node },
                { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
                { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
            >>::OutQ;
                { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
            ] = [
                $( self.edges.$wout.queue_mut() ),*
            ];

            let in_policies: [$crate::prelude::EdgePolicy;
                { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) }
            ] = [
                $( self.edges.$win.policy() ),*
            ];

            let out_policies: [$crate::prelude::EdgePolicy;
                { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
            ] = [
                $( self.edges.$wout.policy() ),*
            ];

            $crate::prelude::StepContext::<'_,
                { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
                { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) },
                <Self as $crate::prelude::GraphNodeTypes<
                    { $w_node },
                    { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
                    { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
                >>::InP,
                <Self as $crate::prelude::GraphNodeTypes<
                    { $w_node },
                    { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
                    { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
                >>::OutP,
                <Self as $crate::prelude::GraphNodeTypes<
                    { $w_node },
                    { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
                    { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
                >>::InQ,
                <Self as $crate::prelude::GraphNodeTypes<
                    { $w_node },
                    { $crate::define_graph!(@node_field in_ports  $w_node ; $NODE_MAP ) },
                    { $crate::define_graph!(@node_field out_ports $w_node ; $NODE_MAP ) }
                >>::OutQ,
                C, T
            >::new(inputs, outputs, in_policies, out_policies, clock, telemetry)
                }
            }
        )*
    };

    // helpers
    (@count $($tt:tt)*) =>
    {
        <[()]>::len(&[ $( { let _ = stringify!($tt); () } ),* ])
    };

     // --- accept bracketed node_map and forward to your existing flat rules ---
    (@node_field $field:ident $want:tt ;
        [ $( $idx:tt : { $($body:tt)* } ),* $(,)? ]
    ) => {
        $crate::define_graph!(
            @node_field $field $want ;
            $( $idx : { $($body)* } ),*
        )
    };
    // node field lookup
    (@node_field ty $want:tt ; $want2:tt :
        {
            ty: $nty:ty,
            in_ports: $nin:expr,
            out_ports: $nout:expr,
            in_payload: $in_p:ty,
            out_payload: $out_p:ty,
            name: $nm:expr
        }
        $(, $rest:tt : { $($rt:tt)* } )*
    ) =>
        {
            $nty
        };
    (@node_field in_ports $want:tt ; $want2:tt :
        {
            ty: $nty:ty,
            in_ports: $nin:expr,
            out_ports: $nout:expr,
            in_payload: $in_p:ty,
            out_payload: $out_p:ty,
            name: $nm:expr
        }
        $(, $rest:tt : { $($rt:tt)* } )*
    ) => {
        $nin
    };
    (@node_field out_ports $want:tt ; $want2:tt :
        {
            ty: $nty:ty,
            in_ports: $nin:expr,
            out_ports: $nout:expr,
            in_payload: $in_p:ty,
            out_payload: $out_p:ty,
            name: $nm:expr
        }
        $(, $rest:tt : { $($rt:tt)* } )*
    ) => {
        $nout
    };
    (@node_field in_payload $want:tt ; $want2:tt :
        {
            ty: $nty:ty,
            in_ports: $nin:expr,
            out_ports: $nout:expr,
            in_payload: $in_p:ty,
            out_payload: $out_p:ty,
            name: $nm:expr
        }
        $(, $rest:tt : { $($rt:tt)* } )*
    ) => {
        $in_p
    };
    (@node_field out_payload $want:tt ; $want2:tt :
        {
            ty: $nty:ty,
            in_ports: $nin:expr,
            out_ports: $nout:expr,
            in_payload: $in_p:ty,
            out_payload: $out_p:ty,
            name: $nm:expr
        }
        $(, $rest:tt : { $($rt:tt)* } )*
    ) => {
        $out_p
    };
    (@node_field $field:ident $want:tt ; $head:tt :
        { $($h:tt)* } , $( $tail:tt : { $($t:tt)* } ),+
    ) => {
        $crate::define_graph!(
            @node_field $field $want ; $( $tail : { $($t)* } ),+ )
    };


    // Accept a bracketed edge-id → queue-type map as one tt group.
    (@in_q_ty $Payload:ty ; [ $first:tt $(, $rest:tt )* ] ; [ $( $eidx:tt => $qty:ty ),* ]) => {
        $crate::define_graph!(@edge_q_ty $first ; [ $( $eidx => $qty ),* ])
    };
    (@in_q_ty $Payload:ty ; [ ] ; [ $( $eidx:tt => $qty:ty ),* ]) => {
        $crate::prelude::NoQueue<$Payload>
    };
    (@out_q_ty $Payload:ty ; [ $first:tt $(, $rest:tt )* ] ; [ $( $eidx:tt => $qty:ty ),* ]) => {
        $crate::define_graph!(@edge_q_ty $first ; [ $( $eidx => $qty ),* ])
    };
    (@out_q_ty $Payload:ty ; [ ] ; [ $( $eidx:tt => $qty:ty ),* ]) => {
        $crate::prelude::NoQueue<$Payload>
    };
    (@edge_q_ty $want:tt ; [ $want2:tt => $qty2:ty $(, $tail:tt => $qtail:ty )* ]) => { $qty2 };
    (@edge_q_ty $want:tt ; [ $head:tt => $qhead:ty $(, $tail:tt => $qtail:ty )+ ]) => {
        $crate::define_graph!(@edge_q_ty $want ; [ $( $tail => $qtail ),+ ])
    };
}

#[cfg(test)]
mod tests {
    use crate::message::MessageFlags;
    use crate::node::NodeCapabilities;
    use crate::node::NodePolicy;
    use crate::policy::AdmissionPolicy;
    use crate::policy::EdgePolicy;
    use crate::policy::OverBudgetAction;
    use crate::policy::QueueCaps;
    use crate::prelude::spsc_array::StaticRing;
    use crate::prelude::GraphApi;
    use crate::prelude::GraphNodeTypes;
    use crate::prelude::Message;
    use crate::prelude::PlacementAcceptance;
    use crate::prelude::SequenceNumber;
    use crate::prelude::SpscQueue;
    use crate::prelude::TestIdentityModelNodeU32;
    use crate::prelude::TestSinkNodeU32;
    use crate::prelude::TestSourceNodeU32;
    use crate::prelude::Ticks;
    use crate::prelude::TraceId;
    use crate::types::QoSClass;

    // Concrete queue type for Message<u32> with a small fixed capacity.
    type Q32 = StaticRing<Message<u32>, 8>;

    const Q_32_POLICY: EdgePolicy = EdgePolicy {
        caps: QueueCaps {
            max_items: 8,
            soft_items: 8,
            max_bytes: None,
            soft_bytes: None,
        },
        over_budget: OverBudgetAction::Drop,
        admission: AdmissionPolicy::DropOldest,
    };

    // ---------- Build a tiny graph with the macro ----------
    define_graph! {
        pub struct TestPipeline;

        nodes {
            0: { ty: TestSourceNodeU32, in_ports: 0, out_ports: 1, in_payload: (),  out_payload: u32, name: Some("src") },
            1: { ty: TestIdentityModelNodeU32, in_ports: 1, out_ports: 1, in_payload: u32, out_payload: u32, name: Some("map") },
            2: { ty: TestSinkNodeU32,   in_ports: 1, out_ports: 0, in_payload: u32, out_payload: (),  name: Some("snk") }
        }

        edges {
            0: { ty: Q32, payload: u32, from: (0,0), to: (1,0), policy: Q_32_POLICY, name: Some("e0") },
            1: { ty: Q32, payload: u32, from: (1,0), to: (2,0), policy: Q_32_POLICY, name: Some("e1") }
        }

        wiring {
            node 0: { in: [ ],   out: [ 0 ] },
            node 1: { in: [ 0 ], out: [ 1 ] },
            node 2: { in: [ 1 ], out: [ ] }
        }
    }

    // Helper trait for compile-time type equality checks.
    trait Same<T> {}
    impl<T> Same<T> for T {}

    fn test_printer(_s: &str) {
        // no_std-friendly sink; ignore in tests
    }

    #[test]
    fn macro_smoke_builds_and_describes() {
        // Build real nodes.
        let src = TestSourceNodeU32::new(
            0,                                  // starting_value_inclusive
            TraceId(0),                         // trace_id
            SequenceNumber(0),                  // starting_sequence
            Ticks(0),                           // starting_tick
            None,                               // deadline_ns
            crate::types::QoSClass::BestEffort, // qos
            crate::message::MessageFlags::empty(),
            crate::node::NodeCapabilities::default(),
            crate::node::NodePolicy::default(),
            [PlacementAcceptance::default()],
        );

        let map = TestIdentityModelNodeU32::new(
            crate::node::NodeCapabilities::default(),
            crate::node::NodePolicy::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        );

        let snk = TestSinkNodeU32::new(
            crate::node::NodeCapabilities::default(),
            crate::node::NodePolicy::default(),
            [PlacementAcceptance::default()],
            test_printer,
        );

        // Build queues.
        let q0: Q32 = StaticRing::new();
        let q1: Q32 = StaticRing::new();

        // Build graph (nodes by index, then edges by index)
        let g = TestPipeline::new(src, map, snk, q0, q1);

        // Descriptors exist and have expected counts
        let nds = <TestPipeline as GraphApi<3, 2>>::get_node_descriptors(&g);
        let eds = <TestPipeline as GraphApi<3, 2>>::get_edge_descriptors(&g);
        assert_eq!(nds.len(), 3);
        assert_eq!(eds.len(), 2);

        // Basic label sanity (if your descriptors expose names)
        assert_eq!(nds[0].name, Some("src"));
        assert_eq!(nds[1].name, Some("map"));
        assert_eq!(nds[2].name, Some("snk"));
        assert_eq!(eds[0].name, Some("e0"));
        assert_eq!(eds[1].name, Some("e1"));

        // Accessors compile and return references
        let _n1 = <TestPipeline as GraphApi<3, 2>>::get_node_ref::<1>(&g);
        let _e0 = <TestPipeline as GraphApi<3, 2>>::get_edge_ref::<0>(&g);

        // Mutable accessors compile
        let src = TestSourceNodeU32::new(
            0,
            TraceId(0),
            SequenceNumber(0),
            Ticks(0),
            None,
            QoSClass::BestEffort,
            MessageFlags::empty(),
            NodeCapabilities::default(),
            NodePolicy::default(),
            [PlacementAcceptance::default()],
        );
        let map = TestIdentityModelNodeU32::new(
            NodeCapabilities::default(),
            NodePolicy::default(),
            [PlacementAcceptance::default()],
            [PlacementAcceptance::default()],
        );
        let snk = TestSinkNodeU32::new(
            NodeCapabilities::default(),
            NodePolicy::default(),
            [PlacementAcceptance::default()],
            test_printer,
        );
        let q0: Q32 = StaticRing::new();
        let q1: Q32 = StaticRing::new();
        let mut g2 = TestPipeline::new(src, map, snk, q0, q1);

        let _n1m = <TestPipeline as GraphApi<3, 2>>::get_node_mut::<1>(&mut g2);
        let _e0m = <TestPipeline as GraphApi<3, 2>>::get_edge_mut::<0>(&mut g2);
    }

    #[test]
    fn compile_time_node_type_resolutions() {
        // Prove that for node 1: IN=1, OUT=1, payloads are u32->u32,
        // and the queues come from the wired edges (i.e., Q32).
        type InP = <TestPipeline as GraphNodeTypes<1, 1, 1>>::InP;
        type OutP = <TestPipeline as GraphNodeTypes<1, 1, 1>>::OutP;
        type InQ = <TestPipeline as GraphNodeTypes<1, 1, 1>>::InQ;
        type OutQ = <TestPipeline as GraphNodeTypes<1, 1, 1>>::OutQ;

        fn _assert_types()
        where
            InP: Same<u32>,
            OutP: Same<u32>,
            InQ: SpscQueue<Item = Message<u32>>,
            OutQ: SpscQueue<Item = Message<u32>>,
        {
        }

        _assert_types();
    }
}
