//! Semantic validator for parsed Limen graph definitions.
//!
//! This module performs *cross-item* checks on a fully parsed [`GraphDef`],
//! catching structural mistakes that are outside the scope of the parser:
//!
//! - **Index continuity**: node and edge indices must be contiguous `0..N`.
//! - **Port bounds**: every edge endpoint must reference valid node ports.
//! - **Payload agreement**: edge payload type must match the upstream node's
//!   `out_payload` and the downstream node's `in_payload`.
//! - **Queue uniformity (per node)**:
//!   - All *input* queues that feed a node must be of the **same queue type**.
//!   - All *output* queues that leave a node must be of the **same queue type**.
//! - **Manager uniformity (per node)**:
//!   - All *input* edges that feed a node must use the **same manager type**.
//!   - All *output* edges that leave a node must use the **same manager type**.
//!
//! The validator reports the *first* encountered error as a `String`. It is
//! expected to be called after parsing (see `crate::parse`) and before code
//! generation (see `crate::gen`).

use crate::ast::GraphDef;

use std::collections::BTreeSet;

/// Validate a parsed [`GraphDef`] for structural and semantic consistency.
///
/// # Checks performed
/// 1. **Index continuity**:
///    - Node indices are unique and exactly `0..nodes.len()-1`.
///    - Edge indices are unique and exactly `0..edges.len()-1`.
/// 2. **Edge endpoint bounds**:
///    - `from_port < from_node.out_ports`
///    - `to_port   < to_node.in_ports`
/// 3. **Payload agreement**:
///    - `edge.payload == from_node.out_payload == to_node.in_payload`
/// 4. **Queue uniformity per node**:
///    - All inbound edges to a given node use the same queue type.
///    - All outbound edges from a given node use the same queue type.
/// 5. **Manager uniformity per node**:
///    - All inbound edges to a given node use the same manager type.
///    - All outbound edges from a given node use the same manager type.
///
/// # Errors
/// Returns `Err(String)` describing the first violation encountered. If all
/// checks pass, returns `Ok(())`.
pub fn validate_definition(g: &GraphDef) -> Result<(), String> {
    // --- Index continuity: nodes ---
    let mut node_ix = BTreeSet::new();
    for n in &g.nodes {
        if !node_ix.insert(n.idx) {
            return Err(format!("duplicate node index {}", n.idx));
        }
    }
    if node_ix.len() != g.nodes.len() || !node_ix.iter().copied().enumerate().all(|(i, x)| i == x) {
        return Err("node indexes must be 0..nodes.len()-1".into());
    }

    // --- Index continuity: edges ---
    let mut edge_ix = BTreeSet::new();
    for e in &g.edges {
        if !edge_ix.insert(e.idx) {
            return Err(format!("duplicate edge index {}", e.idx));
        }
    }
    if edge_ix.len() != g.edges.len() || !edge_ix.iter().copied().enumerate().all(|(i, x)| i == x) {
        return Err("edge indexes must be 0..edges.len()-1".into());
    }

    use quote::ToTokens;
    use std::collections::BTreeMap;

    // Track per-node queue types to enforce uniformity:
    // - in_q_by_node[node_idx] = queue type name of inbound edges
    // - out_q_by_node[node_idx] = queue type name of outbound edges
    let mut in_q_by_node: BTreeMap<usize, String> = BTreeMap::new();
    let mut out_q_by_node: BTreeMap<usize, String> = BTreeMap::new();

    // Track per-node manager types to enforce uniformity:
    // - in_m_by_node[node_idx] = manager type name of inbound edges
    // - out_m_by_node[node_idx] = manager type name of outbound edges
    let mut in_m_by_node: BTreeMap<usize, String> = BTreeMap::new();
    let mut out_m_by_node: BTreeMap<usize, String> = BTreeMap::new();

    for e in &g.edges {
        // --- Port bounds ---
        let from = g
            .nodes
            .get(e.from_node)
            .ok_or_else(|| format!("edge {} from.node out of range", e.idx))?;
        if e.from_port >= from.out_ports {
            return Err(format!(
                "edge {} from.port {} >= node{}.out_ports {}",
                e.idx, e.from_port, e.from_node, from.out_ports
            ));
        }
        let to = g
            .nodes
            .get(e.to_node)
            .ok_or_else(|| format!("edge {} to.node out of range", e.idx))?;
        if e.to_port >= to.in_ports {
            return Err(format!(
                "edge {} to.port {} >= node{}.in_ports {}",
                e.idx, e.to_port, e.to_node, to.in_ports
            ));
        }

        // --- Payload agreement ---
        let e_payload = e.payload.to_token_stream().to_string();
        let from_out = from.out_payload.to_token_stream().to_string();
        let to_in = to.in_payload.to_token_stream().to_string();
        if e_payload != from_out {
            return Err(format!(
                "edge {} payload {:?} != node{}.out_payload {:?}",
                e.idx, e_payload, e.from_node, from_out
            ));
        }
        if e_payload != to_in {
            return Err(format!(
                "edge {} payload {:?} != node{}.in_payload {:?}",
                e.idx, e_payload, e.to_node, to_in
            ));
        }

        // --- Queue uniformity per node ---
        let qn = e.ty.to_token_stream().to_string();
        in_q_by_node
            .entry(e.to_node)
            .and_modify(|s| {
                if *s != qn {
                    *s = "__MISMATCH__".into()
                }
            })
            .or_insert(qn.clone());
        out_q_by_node
            .entry(e.from_node)
            .and_modify(|s| {
                if *s != qn {
                    *s = "__MISMATCH__".into()
                }
            })
            .or_insert(qn);

        // --- Manager uniformity per node ---
        let mn = e.manager_ty.to_token_stream().to_string();
        in_m_by_node
            .entry(e.to_node)
            .and_modify(|s| {
                if *s != mn {
                    *s = "__MISMATCH__".into()
                }
            })
            .or_insert(mn.clone());
        out_m_by_node
            .entry(e.from_node)
            .and_modify(|s| {
                if *s != mn {
                    *s = "__MISMATCH__".into()
                }
            })
            .or_insert(mn);
    }

    // Finalize queue-type uniformity checks.
    for (i, s) in in_q_by_node {
        if s == "__MISMATCH__" {
            return Err(format!("node {} has non-uniform input queue types", i));
        }
    }
    for (i, s) in out_q_by_node {
        if s == "__MISMATCH__" {
            return Err(format!("node {} has non-uniform output queue types", i));
        }
    }

    for (i, s) in in_m_by_node {
        if s == "__MISMATCH__" {
            return Err(format!("node {} has non-uniform input manager types", i));
        }
    }
    for (i, s) in out_m_by_node {
        if s == "__MISMATCH__" {
            return Err(format!("node {} has non-uniform output manager types", i));
        }
    }

    // --- Ingress policy requirement for source nodes ---
    // Project invariant: every source node (in_ports == 0 && out_ports > 0)
    // must expose an external ingress link, which is configured via `ingress_policy`.
    // Enforce this explicitly so that codegen can assume it is always present.
    for n in &g.nodes {
        let is_source = n.in_ports == 0 && n.out_ports > 0;
        if is_source && n.ingress_policy_opt.is_none() {
            return Err(format!(
                "source node {} must declare `ingress_policy` to expose external ingress",
                n.idx
            ));
        }
    }

    Ok(())
}
