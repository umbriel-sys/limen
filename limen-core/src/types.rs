#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, collections::BTreeMap, string::String, vec::Vec};

#[cfg(feature = "alloc")]
use alloc::borrow::Cow;

use core::fmt;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum DataType {
    Boolean,
    Unsigned8,
    Unsigned16,
    Unsigned32,
    Unsigned64,
    Signed8,
    Signed16,
    Signed32,
    Signed64,
    Float16,
    BFloat16,
    Float32,
    Float64,
}

impl DataType {
    pub fn byte_size(&self) -> usize {
        match self {
            DataType::Boolean => 1,
            DataType::Unsigned8 => 1,
            DataType::Signed8 => 1,
            DataType::Unsigned16 => 2,
            DataType::Signed16 => 2,
            DataType::Unsigned32 => 4,
            DataType::Signed32 => 4,
            DataType::Unsigned64 => 8,
            DataType::Signed64 => 8,
            DataType::Float16 => 2,
            DataType::BFloat16 => 2,
            DataType::Float32 => 4,
            DataType::Float64 => 8,
        }
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use DataType::*;
        let s = match self {
            Boolean => "Boolean",
            Unsigned8 => "Unsigned8",
            Unsigned16 => "Unsigned16",
            Unsigned32 => "Unsigned32",
            Unsigned64 => "Unsigned64",
            Signed8 => "Signed8",
            Signed16 => "Signed16",
            Signed32 => "Signed32",
            Signed64 => "Signed64",
            Float16 => "Float16",
            BFloat16 => "BFloat16",
            Float32 => "Float32",
            Float64 => "Float64",
        };
        write!(f, "{}", s)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SequenceNumber(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct TimestampNanosecondsSinceUnixEpoch(pub u128);

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SensorSampleMetadata {
    pub channel_count: Option<u16>,
    pub sample_rate_hz: Option<u32>,
    #[cfg(feature = "alloc")]
    pub notes: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SensorData<'a> {
    pub timestamp: TimestampNanosecondsSinceUnixEpoch,
    pub sequence_number: SequenceNumber,
    pub payload: Cow<'a, [u8]>,
    pub metadata: Option<SensorSampleMetadata>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct QuantizationParams {
    pub scale: f32,
    pub zero_point: i32,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ModelPort {
    #[cfg(feature = "alloc")]
    pub name: Option<String>,
    pub data_type: DataType,
    pub shape: Box<[usize]>,
    pub quantization: Option<QuantizationParams>,
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ModelMetadata {
    #[cfg(feature = "alloc")]
    pub inputs: Vec<ModelPort>,
    #[cfg(feature = "alloc")]
    pub outputs: Vec<ModelPort>,
}

impl ModelMetadata {
    pub fn empty() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TensorInput {
    pub data_type: DataType,
    pub shape: Box<[usize]>,
    pub strides: Option<Box<[usize]>>,
    pub buffer: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct TensorOutput {
    pub data_type: DataType,
    pub shape: Box<[usize]>,
    pub strides: Option<Box<[usize]>>,
    pub buffer: Vec<u8>,
}

fn product(shape: &[usize]) -> Option<usize> {
    let mut p: usize = 1;
    for &d in shape.iter() {
        if d == 0 {
            return None;
        }
        p = p.checked_mul(d)?;
    }
    Some(p)
}

impl TensorInput {
    pub fn new(
        data_type: DataType,
        shape: Box<[usize]>,
        strides: Option<Box<[usize]>>,
        buffer: Vec<u8>,
    ) -> Result<Self, alloc::string::String> {
        let elem = data_type.byte_size();
        let expected = product(&shape)
            .ok_or_else(|| alloc::string::String::from("invalid shape"))?
            .checked_mul(elem)
            .ok_or_else(|| alloc::string::String::from("size overflow"))?;
        if expected != buffer.len() {
            return Err(alloc::string::String::from(
                "buffer length does not match shape * dtype",
            ));
        }
        Ok(Self {
            data_type,
            shape,
            strides,
            buffer,
        })
    }
}

impl TensorOutput {
    pub fn new(
        data_type: DataType,
        shape: Box<[usize]>,
        strides: Option<Box<[usize]>>,
        buffer: Vec<u8>,
    ) -> Result<Self, alloc::string::String> {
        let elem = data_type.byte_size();
        let expected = product(&shape)
            .ok_or_else(|| alloc::string::String::from("invalid shape"))?
            .checked_mul(elem)
            .ok_or_else(|| alloc::string::String::from("size overflow"))?;
        if expected != buffer.len() {
            return Err(alloc::string::String::from(
                "buffer length does not match shape * dtype",
            ));
        }
        Ok(Self {
            data_type,
            shape,
            strides,
            buffer,
        })
    }
}
