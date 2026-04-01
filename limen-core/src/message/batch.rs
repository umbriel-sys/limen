//! Limen-core batch types and helpers.
//!
//! This module provides:
//! - `Batch<'a, P>`: a thin, immutable view over a slice of `Message<P>` used
//!   by policies, telemetry and nodes, and
//! - `BatchView<'a, P>`: an internal container (owned or borrowed) used by the
//!   runtime / NodeLink / StepContext to assemble batches before handing a
//!   borrowed `Batch<'a, P>` to nodes.
//!
//! `BatchView` intentionally provides both an owned (`Vec`) variant (alloc)
//! and a borrowed, stack/heapless-friendly variant so the runtime can operate
//! in both `alloc` and `no-alloc` builds.

use crate::{
    memory::BufferDescriptor,
    message::{payload::Payload, Message, MessageHeader},
    prelude::MemoryManager,
    types::MessageToken,
};

use core::{mem, slice};

/// A thin batch view over a slice of messages.
///
/// Batch formation is runtime-specific; the core only provides
/// a convenient immutable view for policies and telemetry.
#[derive(Debug, Copy, Clone)]
pub struct Batch<'a, P: Payload> {
    /// The ordered messages in the batch.
    messages: &'a [Message<P>],
}

impl<'a, P: Payload> Batch<'a, P> {
    /// Construct a new batch view over a slice of messages.
    #[inline]
    pub const fn new(messages: &'a [Message<P>]) -> Self {
        Self { messages }
    }

    /// Return the underlying messages slice.
    #[inline]
    pub fn messages(&self) -> &'a [Message<P>] {
        self.messages
    }

    /// Return the number of messages in the batch.
    #[inline]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Return `true` if the batch is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Total byte size across message payloads.
    pub fn total_payload_bytes(&self) -> usize {
        self.messages
            .iter()
            .map(|m| m.header.payload_size_bytes)
            .sum()
    }

    /// Iterate over messages.
    #[inline]
    pub fn iter(&self) -> core::slice::Iter<'_, Message<P>> {
        self.messages.iter()
    }

    /// Convenience: is the first message marked FIRST_IN_BATCH (if present)?
    #[inline]
    pub fn first_flagged(&self) -> bool {
        self.messages
            .first()
            .map(|m| m.header.flags.is_first())
            .unwrap_or(false)
    }

    /// Convenience: is the last message marked LAST_IN_BATCH (if present)?
    #[inline]
    pub fn last_flagged(&self) -> bool {
        self.messages
            .last()
            .map(|m| m.header.flags.is_last())
            .unwrap_or(false)
    }

    /// (Optional) Validate flags are consistent with batch boundaries.
    /// Enable only when you want assertions (e.g., in tests) via a feature flag.
    // #[cfg(feature = "validate_batches")]
    #[inline]
    pub fn assert_flags_consistent(&self) {
        if self.is_empty() {
            return;
        }
        debug_assert!(
            self.first_flagged(),
            "batch: first item missing FIRST_IN_BATCH"
        );
        debug_assert!(
            self.last_flagged(),
            "batch: last item missing LAST_IN_BATCH"
        );
        // Optional: internal items should have neither FIRST nor LAST
        for m in &self.messages[1..self.messages.len().saturating_sub(1)] {
            debug_assert!(
                !m.header.flags.is_first() && !m.header.flags.is_last(),
                "batch: internal item has boundary flag"
            );
        }
    }
}

impl<'a, P: Payload> Payload for Batch<'a, P> {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        // Sum payload bytes across messages and add header size per message.
        let total_payload_bytes: usize = self
            .messages
            .iter()
            .map(|m| {
                // Use the header field stored on message as the authoritative payload size.
                // This avoids re-inspecting m.payload() which might be expensive for some payloads.
                m.header.payload_size_bytes
            })
            .sum();

        let header_bytes = self.messages.len() * mem::size_of::<MessageHeader>();
        BufferDescriptor::new(total_payload_bytes + header_bytes)
    }
}

// Provide also for borrowed Batch reference to match other Payload impls pattern.
impl<'a, P: Payload> Payload for &'a Batch<'a, P> {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        (*self).buffer_descriptor()
    }
}

/// An internal batch container used by the runtime/nodelink/stepcontext.
///
/// Generic over the stored item `I`. Commonly `I = Message<P>`, but by using a
/// stored-item generic the same type works for any Edge whose `Item` is a
/// `Payload`-implementing type.
///
/// - `Owned(Vec<I>)`: when `alloc` feature is enabled and we can own a Vec.
/// - `Borrowed(&'a mut [I], len)`: stack/heapless-backed buffer with explicit length.
#[derive(Debug)]
pub enum BatchView<'a, I> {
    /// Owned variant (alloc-enabled). Stores the entire `Vec<I>`.
    #[cfg(feature = "alloc")]
    Owned(alloc::vec::Vec<I>),

    /// Borrowed variant: a mutable slice plus a length indicating the valid prefix.
    Borrowed(&'a mut [I], usize),
}

impl<'a, I> BatchView<'a, I> {
    /// Construct from an owned Vec (alloc feature required).
    #[cfg(feature = "alloc")]
    #[inline]
    pub fn from_owned(v: alloc::vec::Vec<I>) -> Self {
        BatchView::Owned(v)
    }

    /// Construct from a borrowed slice + length.
    #[inline]
    pub fn from_borrowed(buf: &'a mut [I], len: usize) -> Self {
        debug_assert!(len <= buf.len());
        BatchView::Borrowed(buf, len)
    }

    /// Number of items in the batch.
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            #[cfg(feature = "alloc")]
            BatchView::Owned(v) => v.len(),
            BatchView::Borrowed(_, n) => *n,
        }
    }

    /// Is the batch empty?
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Immutable iterator over items.
    #[inline]
    pub fn iter(&self) -> slice::Iter<'_, I> {
        match self {
            #[cfg(feature = "alloc")]
            BatchView::Owned(v) => v.as_slice().iter(),
            BatchView::Borrowed(buf, n) => buf[..*n].iter(),
        }
    }

    /// Mutable iterator over items.
    #[inline]
    pub fn iter_mut(&mut self) -> slice::IterMut<'_, I> {
        match self {
            #[cfg(feature = "alloc")]
            BatchView::Owned(v) => v.as_mut_slice().iter_mut(),
            BatchView::Borrowed(buf, n) => buf[..*n].iter_mut(),
        }
    }

    /// Return an immutable slice over the valid items.
    #[inline]
    pub fn as_slice(&self) -> &[I] {
        match self {
            #[cfg(feature = "alloc")]
            BatchView::Owned(v) => v.as_slice(),
            BatchView::Borrowed(buf, n) => &buf[..*n],
        }
    }
}

/// Special-case helpers for the common stored-item type: `Message<P>`.
impl<'a, P: Payload> BatchView<'a, Message<P>> {
    /// Convert to the public, borrowed `Batch<'_, P>` view.
    ///
    /// This is only available when the stored item is `Message<P>`.
    #[inline]
    pub fn as_batch(&self) -> Batch<'_, P> {
        let slice: &[Message<P>] = match self {
            #[cfg(feature = "alloc")]
            BatchView::Owned(v) => v.as_slice(),
            BatchView::Borrowed(buf, n) => &buf[..*n],
        };
        Batch::new(slice)
    }

    /// Mutable access to the first message header if present.
    #[inline]
    pub fn first_header_mut(&mut self) -> Option<&mut MessageHeader> {
        if self.is_empty() {
            return None;
        }
        Some(match self {
            #[cfg(feature = "alloc")]
            BatchView::Owned(v) => v[0].header_mut(),
            BatchView::Borrowed(buf, _) => buf[0].header_mut(),
        })
    }

    /// Mutable access to the last message header if present.
    #[inline]
    pub fn last_header_mut(&mut self) -> Option<&mut MessageHeader> {
        let n = self.len();
        if n == 0 {
            return None;
        }
        Some(match self {
            #[cfg(feature = "alloc")]
            BatchView::Owned(v) => v[n - 1].header_mut(),
            BatchView::Borrowed(buf, _) => buf[n - 1].header_mut(),
        })
    }

    /// Try to convert into a borrowed `Batch<'_, P>` while keeping `self` alive.
    /// Equivalent to `self.as_batch()` but offered for clarity.
    #[inline]
    pub fn into_batch_ref(&self) -> Batch<'_, P> {
        self.as_batch()
    }

    /// Convert this batch view into an owned batch view.
    ///
    /// This is required by mutex-backed edges: a borrowed batch cannot escape the lock guard.
    ///
    /// Semantics:
    /// - If already `Owned`, the inner `Vec` is forwarded.
    /// - If `Borrowed`, the valid prefix is cloned into a new `Vec`.
    ///
    /// The returned `BatchView` is `Owned` and does not borrow from the original `'a`.
    #[cfg(feature = "alloc")]
    #[inline]
    pub fn into_owned<'b>(self) -> BatchView<'b, Message<P>>
    where
        Message<P>: Clone,
    {
        match self {
            BatchView::Owned(v) => BatchView::<'b, Message<P>>::Owned(v),
            BatchView::Borrowed(buf, n) => {
                let mut v: alloc::vec::Vec<Message<P>> = alloc::vec::Vec::with_capacity(n);
                for m in &buf[..n] {
                    v.push(m.clone());
                }
                BatchView::<'b, Message<P>>::Owned(v)
            }
        }
    }

    /// Consume and return an owned `Vec<Message<P>>`.
    ///
    /// - If this `BatchView` is `Owned`, returns the inner Vec without copying.
    /// - If this `BatchView` is `Borrowed`, clones the valid prefix into a new Vec.
    ///
    /// Alloc-only.
    #[cfg(feature = "alloc")]
    #[inline]
    pub fn into_vec(self) -> alloc::vec::Vec<Message<P>>
    where
        P: Clone,
    {
        match self {
            BatchView::Owned(v) => v,
            BatchView::Borrowed(buf, n) => {
                let mut v = alloc::vec::Vec::with_capacity(n);
                for m in &buf[..n] {
                    v.push(m.clone());
                }
                v
            }
        }
    }
}

impl<'a, P: Payload> Payload for BatchView<'a, Message<P>> {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        match self {
            #[cfg(feature = "alloc")]
            BatchView::Owned(v) => {
                let total_payload_bytes: usize =
                    v.iter().map(|m| m.header().payload_size_bytes).sum();
                let header_bytes = v.len() * mem::size_of::<MessageHeader>();
                BufferDescriptor::new(total_payload_bytes + header_bytes)
            }

            BatchView::Borrowed(buf, n) => {
                let total_payload_bytes: usize = buf[..*n]
                    .iter()
                    .map(|m| m.header().payload_size_bytes)
                    .sum();
                let header_bytes = *n * mem::size_of::<MessageHeader>();
                BufferDescriptor::new(total_payload_bytes + header_bytes)
            }
        }
    }
}

// Borrowed ref as well
impl<'a, P: Payload> Payload for &'a BatchView<'a, Message<P>> {
    #[inline]
    fn buffer_descriptor(&self) -> BufferDescriptor {
        (*self).buffer_descriptor()
    }
}

/// Lazy guard-yielding iterator over messages in a batch.
///
/// Backed by token resolution through a memory manager. Each `next()` call
/// takes a `ReadGuard` from the manager — no copying, no contiguous buffer
/// required.
///
/// Nodes can:
/// - iterate and process one at a time (default `step_batch`)
/// - iterate and copy into a node-owned scratch buffer (`InferenceModel`)
pub struct BatchMessageIter<'edge, 'mgr, P: Payload, M: MemoryManager<P>> {
    tokens: core::slice::Iter<'edge, MessageToken>,
    manager: &'mgr M,
    /// Number of leading tokens that were popped (rest are peeked).
    stride: usize,
    /// Total number of tokens in the batch.
    len: usize,
    _pd: core::marker::PhantomData<P>,
}

impl<'edge, 'mgr, P: Payload, M: MemoryManager<P>> BatchMessageIter<'edge, 'mgr, P, M> {
    /// Construct a new `BatchMessageIter` from token slice, manager, and stride.
    #[inline]
    pub fn new(
        tokens: core::slice::Iter<'edge, MessageToken>,
        manager: &'mgr M,
        stride: usize,
        len: usize,
    ) -> Self {
        Self {
            tokens,
            manager,
            stride,
            len,
            _pd: core::marker::PhantomData,
        }
    }

    /// How many items were popped (will be freed after the callback).
    #[inline]
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// Total batch length (popped + peeked).
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the batch is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether this is a sliding window batch (stride < len).
    #[inline]
    pub fn is_sliding(&self) -> bool {
        self.stride < self.len
    }
}

impl<'edge, 'mgr, P: Payload, M: MemoryManager<P>> Iterator
    for BatchMessageIter<'edge, 'mgr, P, M>
{
    type Item = M::ReadGuard<'mgr>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let &token = self.tokens.next()?;
        self.manager.read(token).ok()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.tokens.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::{create_test_tensor_filled_with, TestTensor, TEST_TENSOR_BYTE_COUNT};

    use super::*;

    /// Helper: construct a `Message<TestTensor>` with an empty header and a
    /// uniformly-filled shared test tensor payload.
    fn make_msg_tensor(v: u32) -> Message<TestTensor> {
        Message::new(MessageHeader::empty(), create_test_tensor_filled_with(v))
    }

    #[test]
    fn batch_basic_props() {
        // Build a small array of messages and create a Batch view.
        let arr: [Message<TestTensor>; 3] = [
            make_msg_tensor(10),
            make_msg_tensor(11),
            make_msg_tensor(12),
        ];

        // Build a Batch directly over a slice.
        let batch = Batch::new(&arr[..2]); // first two items
        assert_eq!(batch.len(), 2);
        assert!(!batch.is_empty());
        assert_eq!(batch.messages().len(), 2);

        // total_payload_bytes should be sum of test tensor payload sizes
        assert_eq!(batch.total_payload_bytes(), 2 * TEST_TENSOR_BYTE_COUNT);

        // initially flags are not set
        assert!(!batch.first_flagged());
        assert!(!batch.last_flagged());
    }

    #[test]
    fn batchview_borrowed_basic_and_mutation() {
        // Prepare 4 messages but claim only first 3 are valid for the batch.
        let mut arr: [Message<TestTensor>; 4] = [
            make_msg_tensor(100),
            make_msg_tensor(101),
            make_msg_tensor(102),
            make_msg_tensor(103),
        ];

        // Create Borrowed BatchView with length = 3
        let mut bv = BatchView::from_borrowed(&mut arr, 3);
        assert_eq!(bv.len(), 3);
        assert!(!bv.is_empty());

        // Mutate payloads via iter_mut()
        for (i, m) in bv.iter_mut().enumerate() {
            *m.payload_mut() = create_test_tensor_filled_with(200 + (i as u32));
        }

        // Convert to public Batch and inspect values without using vec
        let batch = bv.as_batch();
        let mut vals = [TestTensor::default(); 3];
        let mut i = 0;
        for m in batch.iter() {
            vals[i] = m.payload().clone();
            i += 1;
        }
        assert_eq!(
            vals,
            [
                create_test_tensor_filled_with(200),
                create_test_tensor_filled_with(201),
                create_test_tensor_filled_with(202),
            ]
        );

        // Set first/last flags through BatchView helpers
        {
            let fh = bv.first_header_mut().expect("first header");
            fh.set_first_in_batch();
            let lh = bv.last_header_mut().expect("last header");
            lh.set_last_in_batch();
        }

        let batch2 = bv.as_batch();
        assert!(batch2.first_flagged());
        assert!(batch2.last_flagged());
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn batchview_owned_basic_and_into_owned() {
        use alloc::vec::Vec;

        // Create an owned vector of messages.
        let mut vec: Vec<Message<TestTensor>> = Vec::new();
        vec.push(make_msg_tensor(1));
        vec.push(make_msg_tensor(2));
        vec.push(make_msg_tensor(3));

        let mut bv = BatchView::from_owned(vec);
        assert_eq!(bv.len(), 3);
        assert!(!bv.is_empty());

        // Mutate last payload via iter_mut()
        for (i, m) in bv.iter_mut().enumerate() {
            if i == 2 {
                *m.payload_mut() = create_test_tensor_filled_with(42);
            }
        }

        // Inspect via as_batch into an owned Vec
        let batch = bv.as_batch();
        let mut xs: Vec<TestTensor> = Vec::new();
        for m in batch.iter() {
            xs.push(m.payload().clone());
        }
        // compare to a slice instead of using `vec![]` macro
        assert_eq!(
            xs.as_slice(),
            &[
                create_test_tensor_filled_with(1),
                create_test_tensor_filled_with(2),
                create_test_tensor_filled_with(42),
            ]
        );

        // Check header helpers and then consume owned vec
        {
            let fh = bv.first_header_mut().expect("first header");
            fh.set_first_in_batch();
            let lh = bv.last_header_mut().expect("last header");
            lh.set_last_in_batch();
        }
        let batch2 = bv.as_batch();
        assert!(batch2.first_flagged());
        assert!(batch2.last_flagged());

        // Consume the owned vec
        let ov = bv.into_vec();
        assert_eq!(ov.len(), 3);
        // Confirm the final payload value (42) survived.
        assert_eq!(
            *ov.last().unwrap().payload(),
            create_test_tensor_filled_with(42)
        );
    }

    #[test]
    fn batch_assert_flags_consistent_no_panic_when_correct() {
        let mut arr: [Message<TestTensor>; 2] = [make_msg_tensor(7), make_msg_tensor(8)];

        // set flags explicitly on headers and make a Batch
        {
            let mut bv = BatchView::from_borrowed(&mut arr, 2);
            bv.first_header_mut().unwrap().set_first_in_batch();
            bv.last_header_mut().unwrap().set_last_in_batch();
            let batch = bv.as_batch();
            // This should not panic (debug_assert runs in test builds)
            batch.assert_flags_consistent();
        }
    }
}
