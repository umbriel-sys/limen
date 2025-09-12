//! Two-lane priority wrapper for SPSC queues (feature: `priority_lanes`).
//!
//! This composes two underlying SPSC queues (hi/lo) that store the same `Item`
//! and routes `try_push` by inspecting the message header's QoS class. `try_pop`
//! always prefers the high-priority lane when available.

use limen_core::queue::{SpscQueue, EnqueueResult, QueueOccupancy};
use limen_core::policy::{EdgePolicy};
use limen_core::errors::QueueError;
use limen_core::message::{Message, Payload};
use limen_core::types::QoSClass;

/// Two-lane priority queue.
pub struct Priority2<QHi, QLo, P>
where
    QHi: SpscQueue<Item = Message<P>>,
    QLo: SpscQueue<Item = Message<P>>,
    P: Payload,
{
    hi: QHi,
    lo: QLo,
    _p: core::marker::PhantomData<P>,
}

impl<QHi, QLo, P> Priority2<QHi, QLo, P>
where
    QHi: SpscQueue<Item = Message<P>>,
    QLo: SpscQueue<Item = Message<P>>,
    P: Payload,
{
    /// Build a two-lane priority queue from hi/lo queues.
    pub fn new(hi: QHi, lo: QLo) -> Self {
        Self { hi, lo, _p: core::marker::PhantomData }
    }
}

impl<QHi, QLo, P> SpscQueue for Priority2<QHi, QLo, P>
where
    QHi: SpscQueue<Item = Message<P>>,
    QLo: SpscQueue<Item = Message<P>>,
    P: Payload,
{
    type Item = Message<P>;

    fn try_push(&mut self, item: Self::Item, policy: &EdgePolicy) -> EnqueueResult {
        match item.header.qos {
            QoSClass::LatencyCritical => self.hi.try_push(item, policy),
            _ => self.lo.try_push(item, policy),
        }
    }

    fn try_pop(&mut self) -> Result<Self::Item, QueueError> {
        match self.hi.try_pop() {
            Ok(it) => Ok(it),
            Err(QueueError::Empty) => self.lo.try_pop(),
            Err(e) => Err(e),
        }
    }

    fn occupancy(&self, policy: &EdgePolicy) -> QueueOccupancy {
        let hi = self.hi.occupancy(policy);
        let lo = self.lo.occupancy(policy);
        let items = hi.items + lo.items;
        let bytes = hi.bytes + lo.bytes;
        let watermark = policy.watermark(items, bytes);
        QueueOccupancy { items, bytes, watermark }
    }

    fn try_peek(&self) -> Result<&Self::Item, QueueError> {
        match self.hi.try_peek() {
            Ok(r) => Ok(r),
            Err(QueueError::Empty) => self.lo.try_peek(),
            Err(e) => Err(e),
        }
    }
}
