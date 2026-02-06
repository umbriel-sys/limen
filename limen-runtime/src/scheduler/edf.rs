//! Earliest-Deadline-First dequeue policy.
// use limen_core::scheduling::{DequeuePolicy, NodeSummary, Readiness};
// use limen_core::types::NodeIndex;

// /// EDF policy that prioritizes nodes with the earliest absolute deadline.
// #[derive(Debug, Default)]
// pub struct EdfPolicy {
//     rr: usize,
// }

// impl DequeuePolicy for EdfPolicy {
//     fn select_next(&mut self, candidates: &[NodeSummary]) -> Option<NodeIndex> {
//         // Prefer Ready nodes with a deadline, pick the smallest deadline.
//         let mut best: Option<(usize, u64, usize)> = None; // (idx_in_slice, deadline_ns, rr_tie)
//         for (i, c) in candidates.iter().enumerate() {
//             if c.readiness != Readiness::Ready {
//                 continue;
//             }
//             if let Some(d) = c.earliest_deadline {
//                 let d_ns = d.0;
//                 let tie = (self.rr + i) % (candidates.len().max(1));
//                 match best {
//                     None => best = Some((i, d_ns, tie)),
//                     Some((_, bd, bt)) => {
//                         if d_ns < bd || (d_ns == bd && tie < bt) {
//                             best = Some((i, d_ns, tie));
//                         }
//                     }
//                 }
//             }
//         }
//         self.rr = self.rr.wrapping_add(1);
//         best.map(|(i, _, _)| candidates[i].index).or_else(|| {
//             // Fallback: any Ready candidate
//             candidates
//                 .iter()
//                 .position(|c| c.readiness == Readiness::Ready)
//                 .map(|i| candidates[i].index)
//         })
//     }
// }
