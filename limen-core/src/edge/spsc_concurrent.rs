//! Concurrent Queue generic impl, updated for `MessageToken` + `HeaderStore` Edge API.

use std::sync::{Arc, Mutex};

use crate::edge::{Edge, EdgeOccupancy, EnqueueResult};
use crate::errors::QueueError;
use crate::policy::{EdgePolicy, WatermarkState};
use crate::prelude::BatchView;
use crate::prelude::HeaderStore;
use crate::types::MessageToken; // bring HeaderStore into scope

/// Thread-safe wrapper: makes ANY `Q: Edge` cloneable + `Send + 'static`.
pub struct ConcurrentQueue<Q> {
    inner: Arc<Mutex<Q>>,
}

impl<Q> ConcurrentQueue<Q> {
    /// Creates a new ConcurrentQueue from the given queue.
    pub fn new(inner: Q) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Creates a new ConcurrentQueue from the given Arc<Mutex<queue>>.
    pub fn from_arc(inner: Arc<Mutex<Q>>) -> Self {
        Self { inner }
    }

    /// Returns an arc clone of the inner queue.
    pub fn arc(&self) -> Arc<Mutex<Q>> {
        Arc::clone(&self.inner)
    }
}

impl<Q> Clone for ConcurrentQueue<Q> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<Q> ConcurrentQueue<Q>
where
    Q: Edge + Send + 'static,
{
    /// Helper to lock the inner mutex and map poisoning to `QueueError::Poisoned`.
    fn lock_inner(&self) -> Result<std::sync::MutexGuard<'_, Q>, QueueError> {
        self.inner.lock().map_err(|_| QueueError::Poisoned)
    }
}

impl<Q> Edge for ConcurrentQueue<Q>
where
    Q: Edge + Send + 'static,
{
    fn try_push<H: HeaderStore>(
        &mut self,
        token: MessageToken,
        policy: &EdgePolicy,
        headers: &H,
    ) -> EnqueueResult {
        match self.inner.lock() {
            Ok(mut q) => q.try_push(token, policy, headers),
            Err(_) => EnqueueResult::Rejected,
        }
    }

    fn try_pop<H: HeaderStore>(&mut self, headers: &H) -> Result<MessageToken, QueueError> {
        match self.inner.lock() {
            Ok(mut q) => q.try_pop(headers),
            Err(_) => Err(QueueError::Poisoned),
        }
    }

    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
        match self.inner.lock() {
            Ok(q) => q.occupancy(policy),
            Err(_) => EdgeOccupancy {
                items: 0,
                bytes: 0,
                watermark: WatermarkState::AtOrAboveHard,
            },
        }
    }

    fn is_empty(&self) -> bool {
        match self.inner.lock() {
            Ok(q) => q.is_empty(),
            Err(_) => true,
        }
    }

    fn try_peek(&self) -> Result<MessageToken, QueueError> {
        match self.inner.lock() {
            Ok(q) => match q.try_peek() {
                Ok(t) => Ok(t), // MessageToken is cheap to copy/return by value
                Err(e) => Err(e),
            },
            Err(_) => Err(QueueError::Poisoned),
        }
    }

    fn try_peek_at(&self, index: usize) -> Result<MessageToken, QueueError> {
        match self.inner.lock() {
            Ok(q) => match q.try_peek_at(index) {
                Ok(t) => Ok(t),
                Err(e) => Err(e),
            },
            Err(_) => Err(QueueError::Poisoned),
        }
    }

    fn try_pop_batch<H: HeaderStore>(
        &mut self,
        policy: &crate::policy::BatchingPolicy,
        headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError> {
        match self.inner.lock() {
            Ok(mut q) => {
                // Obtain the batch view from inner while holding the lock.
                let batch = q.try_pop_batch(policy, headers)?;

                // Materialize into an owned vector while holding the mutex, so no
                // borrowed references escape the lock guard.
                let mut owned: alloc::vec::Vec<MessageToken> =
                    alloc::vec::Vec::with_capacity(batch.len());

                for &tok in batch.iter() {
                    owned.push(tok);
                }

                Ok(BatchView::from_owned(owned))
            }
            Err(_) => Err(QueueError::Poisoned),
        }
    }
}

/// Producer endpoint: push + occupancy only.
#[derive(Clone)]
pub struct ProducerEndpoint<QWrap>
where
    QWrap: Edge + Send + 'static,
{
    q: QWrap,
}

impl<QWrap> ProducerEndpoint<QWrap>
where
    QWrap: Edge + Send + 'static,
{
    /// Creates a new ProducerEndpoint.
    pub fn new(q: QWrap) -> Self {
        Self { q }
    }

    /// Returns the inner queue.
    pub fn into_inner(self) -> QWrap {
        self.q
    }
}

impl<QWrap> Edge for ProducerEndpoint<QWrap>
where
    QWrap: Edge + Send + 'static,
{
    fn try_push<H: HeaderStore>(
        &mut self,
        token: MessageToken,
        policy: &EdgePolicy,
        headers: &H,
    ) -> EnqueueResult {
        self.q.try_push(token, policy, headers)
    }

    fn try_pop<H: HeaderStore>(&mut self, _headers: &H) -> Result<MessageToken, QueueError> {
        Err(QueueError::Empty)
    }

    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
        self.q.occupancy(policy)
    }

    fn is_empty(&self) -> bool {
        self.q.is_empty()
    }

    fn try_peek(&self) -> Result<MessageToken, QueueError> {
        Err(QueueError::Unsupported)
    }

    fn try_peek_at(&self, _index: usize) -> Result<MessageToken, QueueError> {
        Err(QueueError::Unsupported)
    }

    fn try_pop_batch<H: HeaderStore>(
        &mut self,
        _policy: &crate::policy::BatchingPolicy,
        _headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError> {
        Err(QueueError::Unsupported)
    }
}

/// Consumer endpoint: pop + occupancy + peek only.
#[derive(Clone)]
pub struct ConsumerEndpoint<QWrap>
where
    QWrap: Edge + Send + 'static,
{
    q: QWrap,
}

impl<QWrap> ConsumerEndpoint<QWrap>
where
    QWrap: Edge + Send + 'static,
{
    /// Creates a new ConsumerEndpoint.
    pub fn new(q: QWrap) -> Self {
        Self { q }
    }

    /// Returns the inner queue.
    pub fn into_inner(self) -> QWrap {
        self.q
    }
}

impl<QWrap> Edge for ConsumerEndpoint<QWrap>
where
    QWrap: Edge + Send + 'static,
{
    fn try_push<H: HeaderStore>(
        &mut self,
        _token: MessageToken,
        _policy: &EdgePolicy,
        _headers: &H,
    ) -> EnqueueResult {
        EnqueueResult::Rejected
    }

    fn try_pop<H: HeaderStore>(&mut self, headers: &H) -> Result<MessageToken, QueueError> {
        self.q.try_pop(headers)
    }

    fn occupancy(&self, policy: &EdgePolicy) -> EdgeOccupancy {
        self.q.occupancy(policy)
    }

    fn is_empty(&self) -> bool {
        self.q.is_empty()
    }

    fn try_peek(&self) -> Result<MessageToken, QueueError> {
        self.q.try_peek()
    }

    fn try_peek_at(&self, index: usize) -> Result<MessageToken, QueueError> {
        self.q.try_peek_at(index)
    }

    fn try_pop_batch<H: HeaderStore>(
        &mut self,
        policy: &crate::policy::BatchingPolicy,
        headers: &H,
    ) -> Result<BatchView<'_, MessageToken>, QueueError> {
        self.q.try_pop_batch(policy, headers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::edge::bench::TestSpscRingBuf;

    crate::run_edge_contract_tests!(concurrent_queue_contract, || {
        let inner = TestSpscRingBuf::<16>::new();
        ConcurrentQueue::new(inner)
    });

    mod endpoint_tests {
        use super::*;

        use crate::edge::bench::TestSpscRingBuf;
        use crate::errors::QueueError;
        use crate::memory::manager::MemoryManager;
        use crate::memory::static_manager::StaticMemoryManager;
        use crate::message::{Message, MessageHeader};
        use crate::policy::{AdmissionPolicy, EdgePolicy, OverBudgetAction, QueueCaps};
        use crate::types::{MessageToken, Ticks};

        const POLICY: EdgePolicy = EdgePolicy::new(
            QueueCaps::new(8, 6, None, None),
            AdmissionPolicy::DropNewest,
            OverBudgetAction::Drop,
        );

        const MGR_DEPTH: usize = 16;

        fn make_msg_u32(tick: u64) -> Message<u32> {
            let mut h = MessageHeader::empty();
            h.set_creation_tick(Ticks::new(tick));
            Message::new(h, 0u32)
        }

        #[test]
        fn producer_endpoint_is_write_only() {
            let inner = ConcurrentQueue::new(TestSpscRingBuf::<16>::new());
            let mut producer = ProducerEndpoint::new(inner);
            let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();

            // Push should succeed.
            let m = make_msg_u32(1);
            let token = mgr.store(m).expect("store");
            assert_eq!(
                producer.try_push(token, &POLICY, &mgr),
                EnqueueResult::Enqueued
            );

            // Pop is disabled on producer endpoint.
            assert!(matches!(producer.try_pop(&mgr), Err(QueueError::Empty)));

            // Peek is unsupported on producer endpoint.
            assert!(matches!(producer.try_peek(), Err(QueueError::Unsupported)));
            assert!(matches!(
                producer.try_peek_at(0),
                Err(QueueError::Unsupported)
            ));

            // Occupancy is forwarded.
            let occ = producer.occupancy(&POLICY);
            assert_eq!(*occ.items(), 1usize);
        }

        #[test]
        fn consumer_endpoint_is_read_only() {
            let inner = ConcurrentQueue::new(TestSpscRingBuf::<16>::new());
            let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();

            // Pre-fill the queue via a clone of the ConcurrentQueue before splitting.
            let mut q_clone = inner.clone();
            let m = make_msg_u32(1);
            let token = mgr.store(m).expect("store");
            assert_eq!(
                q_clone.try_push(token, &POLICY, &mgr),
                EnqueueResult::Enqueued
            );

            let mut consumer = ConsumerEndpoint::new(inner);

            // Push is rejected on consumer endpoint.
            let m2 = make_msg_u32(2);
            let token2 = mgr.store(m2).expect("store");
            assert_eq!(
                consumer.try_push(token2, &POLICY, &mgr),
                EnqueueResult::Rejected
            );

            // Occupancy is forwarded (should see 1 item from the push via q_clone).
            let occ = consumer.occupancy(&POLICY);
            assert_eq!(*occ.items(), 1usize);

            // Peek works on consumer.
            let peek_token = consumer.try_peek().expect("consumer peek");
            assert_eq!(peek_token, token);

            // Pop works on consumer.
            let popped = consumer.try_pop(&mgr).expect("consumer pop");
            assert_eq!(popped, token);

            // Now empty.
            assert!(consumer.is_empty());
            assert!(matches!(consumer.try_pop(&mgr), Err(QueueError::Empty)));
        }

        #[test]
        fn concurrent_queue_cross_thread_push_pop() {
            let inner = TestSpscRingBuf::<16>::new();
            let cq = ConcurrentQueue::new(inner);
            let mut mgr: StaticMemoryManager<u32, MGR_DEPTH> = StaticMemoryManager::new();

            // Store several messages and collect tokens.
            let mut tokens = [MessageToken::INVALID; 4];
            for (i, t) in (1u64..=4u64).enumerate() {
                let m = make_msg_u32(t);
                tokens[i] = mgr.store(m).expect("store");
            }

            // Push all tokens via one clone.
            let mut producer = cq.clone();
            for &tok in &tokens {
                assert_eq!(
                    producer.try_push(tok, &POLICY, &mgr),
                    EnqueueResult::Enqueued
                );
            }

            // Pop all tokens via another clone — FIFO order.
            let mut consumer = cq.clone();
            for (i, expected_tick) in (1u64..=4u64).enumerate() {
                let popped = consumer.try_pop(&mgr).expect("pop");
                assert_eq!(popped, tokens[i]);
                let h = mgr.peek_header(popped).expect("header");
                assert_eq!(*h.creation_tick(), Ticks::new(expected_tick));
            }
            assert!(matches!(consumer.try_pop(&mgr), Err(QueueError::Empty)));
        }
    }
}
