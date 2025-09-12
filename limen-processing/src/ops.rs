//! Split/Join operator examples (copy, scatter, concat, sum).
use limen_core::routing::{SplitOperator, JoinOperator};
use limen_core::types::Ticks;
use limen_core::message::Payload;
use crate::payload::Tensor1D;

/// Copy split: duplicate the input into all outputs.
pub struct CopySplit<P: Payload, const N: usize>(core::marker::PhantomData<(P, [(); N])>);

impl<P: Payload, const N: usize> CopySplit<P, N> {
    /// Create a new copy split operator.
    pub const fn new() -> Self { Self(core::marker::PhantomData) }
}

impl<const N: usize, T: Copy, const K: usize> SplitOperator<Tensor1D<T, N>, Tensor1D<T, N>, K> for CopySplit<Tensor1D<T, N>, K> {
    fn split(&mut self, input: &Tensor1D<T, N>, _budget: Option<Ticks>, outputs: &mut [Tensor1D<T, N>; K]) {
        for o in outputs.iter_mut() { *o = *input; }
    }
}

/// Scatter split: divide the input equally across outputs by contiguous segments.
pub struct ScatterSplit<const N: usize, const K: usize>;

impl<const N: usize, const K: usize> SplitOperator<Tensor1D<f32, N>, Tensor1D<f32, {N / K}>, K> for ScatterSplit<N, K> {
    fn split(&mut self, input: &Tensor1D<f32, N>, _budget: Option<Ticks>, outputs: &mut [Tensor1D<f32, {N / K}>; K]) {
        let chunk = N / K;
        for i in 0..K {
            let mut out = [0.0f32; {N / K}];
            for j in 0..chunk {
                out[j] = input.data[i*chunk + j];
            }
            outputs[i] = Tensor1D { data: out };
        }
    }
}

/// Concat join: concatenate K inputs into one output (N per input).
pub struct ConcatJoin<const N: usize, const K: usize>;

impl<const N: usize, const K: usize> JoinOperator<Tensor1D<f32, N>, Tensor1D<f32, {N * K}>, K> for ConcatJoin<N, K> {
    fn join(&mut self, inputs: &[Tensor1D<f32, N>; K], _budget: Option<Ticks>) -> Tensor1D<f32, {N * K}> {
        let mut out = [0.0f32; {N * K}];
        for i in 0..K {
            for j in 0..N {
                out[i*N + j] = inputs[i].data[j];
            }
        }
        Tensor1D { data: out }
    }
}

/// Sum join: element-wise sum across inputs (same shape).
pub struct SumJoin<const N: usize, const K: usize>;

impl<const N: usize, const K: usize> JoinOperator<Tensor1D<f32, N>, Tensor1D<f32, N>, K> for SumJoin<N, K> {
    fn join(&mut self, inputs: &[Tensor1D<f32, N>; K], _budget: Option<Ticks>) -> Tensor1D<f32, N> {
        let mut out = [0.0f32; N];
        for i in 0..K {
            for j in 0..N {
                out[j] += inputs[i].data[j];
            }
        }
        Tensor1D { data: out }
    }
}
