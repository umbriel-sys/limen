use limen_core::errors::InferenceError;
use limen_core::traits::{ComputeBackend, ComputeBackendFactory, Model};
use limen_core::traits::configuration::ComputeBackendConfiguration;
use limen_core::types::{DataType, ModelMetadata, TensorInput, TensorOutput};

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec};

use tract_onnx::prelude::*;

type TractTypedPlan = typed::TypedRunnableModel<typed::Graph<TypedFact, Box<dyn TypedOp>>>;

pub struct TractOnnxModel {
    runnable: TractTypedPlan,
    declared_metadata: ModelMetadata,
}

impl Model for TractOnnxModel {
    fn model_metadata(&self) -> ModelMetadata { self.declared_metadata.clone() }

    fn infer(&mut self, tensor_input: &TensorInput) -> Result<TensorOutput, InferenceError> {
        let shape_usize: Vec<usize> = tensor_input.shape.clone().into_vec();
        if shape_usize.is_empty() {
            return Err(InferenceError::Other { message: "input shape is empty".to_string() });
        }

        let input_tensor = match tensor_input.data_type {
            DataType::Float32 => {
                let v = bytes_to_vec_f32(&tensor_input.buffer)?;
                Tensor::from_shape(&shape_usize, &v).map_err(|_| InferenceError::Other { message: "failed to build f32 tensor".to_string() })?
            }
            DataType::Float64 => {
                let v = bytes_to_vec_f64(&tensor_input.buffer)?;
                Tensor::from_shape(&shape_usize, &v).map_err(|_| InferenceError::Other { message: "failed to build f64 tensor".to_string() })?
            }
            DataType::Unsigned8 => {
                Tensor::from_shape(&shape_usize, &tensor_input.buffer).map_err(|_| InferenceError::Other { message: "failed to build u8 tensor".to_string() })?
            }
            DataType::Signed8 => {
                let v = bytes_to_vec_i8(&tensor_input.buffer)?;
                Tensor::from_shape(&shape_usize, &v).map_err(|_| InferenceError::Other { message: "failed to build i8 tensor".to_string() })?
            }
            DataType::Unsigned16 => {
                let v = bytes_to_vec_u16(&tensor_input.buffer)?;
                Tensor::from_shape(&shape_usize, &v).map_err(|_| InferenceError::Other { message: "failed to build u16 tensor".to_string() })?
            }
            DataType::Signed16 => {
                let v = bytes_to_vec_i16(&tensor_input.buffer)?;
                Tensor::from_shape(&shape_usize, &v).map_err(|_| InferenceError::Other { message: "failed to build i16 tensor".to_string() })?
            }
            DataType::Unsigned32 => {
                let v = bytes_to_vec_u32(&tensor_input.buffer)?;
                Tensor::from_shape(&shape_usize, &v).map_err(|_| InferenceError::Other { message: "failed to build u32 tensor".to_string() })?
            }
            DataType::Signed32 => {
                let v = bytes_to_vec_i32(&tensor_input.buffer)?;
                Tensor::from_shape(&shape_usize, &v).map_err(|_| InferenceError::Other { message: "failed to build i32 tensor".to_string() })?
            }
            DataType::Unsigned64 => {
                let v = bytes_to_vec_u64(&tensor_input.buffer)?;
                Tensor::from_shape(&shape_usize, &v).map_err(|_| InferenceError::Other { message: "failed to build u64 tensor".to_string() })?
            }
            DataType::Signed64 => {
                let v = bytes_to_vec_i64(&tensor_input.buffer)?;
                Tensor::from_shape(&shape_usize, &v).map_err(|_| InferenceError::Other { message: "failed to build i64 tensor".to_string() })?
            }
            DataType::Boolean => {
                let v = bytes_to_vec_bool(&tensor_input.buffer)?;
                Tensor::from_shape(&shape_usize, &v).map_err(|_| InferenceError::Other { message: "failed to build bool tensor".to_string() })?
            }
            other => {
                return Err(InferenceError::Other { message: format!("input data type not supported by tract backend: {:?}", other) })
            }
        };

        let outputs = self.runnable.run(tvec![input_tensor.into()]).map_err(|e| InferenceError::Other { message: format!("tract run failed: {e}") })?;
        if outputs.is_empty() { return Err(InferenceError::Other { message: "tract returned zero outputs".to_string() }); }
        let output_value = &outputs[0];
        let datum_type = output_value.datum_type();
        let output_shape: Vec<usize> = output_value.shape().to_vec();

        let (data_type, buffer) = match datum_type {
            DatumType::F32 => {
                if let Ok(slice) = output_value.as_slice::<f32>() {
                    (DataType::Float32, slice_f32_to_le_bytes(slice))
                } else {
                    let view = output_value.to_array_view::<f32>().map_err(|_| InferenceError::Other { message: "cannot view f32 output".to_string() })?;
                    let mut bytes = Vec::with_capacity(view.len() * core::mem::size_of::<f32>());
                    for &x in view.iter() { bytes.extend_from_slice(&x.to_le_bytes()); }
                    (DataType::Float32, bytes)
                }
            }
            DatumType::F64 => {
                if let Ok(slice) = output_value.as_slice::<f64>() {
                    (DataType::Float64, slice_f64_to_le_bytes(slice))
                } else {
                    let view = output_value.to_array_view::<f64>().map_err(|_| InferenceError::Other { message: "cannot view f64 output".to_string() })?;
                    let mut bytes = Vec::with_capacity(view.len() * core::mem::size_of::<f64>());
                    for &x in view.iter() { bytes.extend_from_slice(&x.to_le_bytes()); }
                    (DataType::Float64, bytes)
                }
            }
            DatumType::U8 => {
                if let Ok(slice) = output_value.as_slice::<u8>() {
                    (DataType::Unsigned8, slice.to_vec())
                } else {
                    let view = output_value.to_array_view::<u8>().map_err(|_| InferenceError::Other { message: "cannot view u8 output".to_string() })?;
                    (DataType::Unsigned8, view.iter().copied().collect())
                }
            }
            DatumType::I8 => {
                if let Ok(slice) = output_value.as_slice::<i8>() {
                    (DataType::Signed8, slice_i8_to_le_bytes(slice))
                } else {
                    let view = output_value.to_array_view::<i8>().map_err(|_| InferenceError::Other { message: "cannot view i8 output".to_string() })?;
                    (DataType::Signed8, view.iter().map(|&x| x as u8).collect())
                }
            }
            DatumType::U16 => {
                if let Ok(slice) = output_value.as_slice::<u16>() {
                    (DataType::Unsigned16, slice_u16_to_le_bytes(slice))
                } else {
                    let view = output_value.to_array_view::<u16>().map_err(|_| InferenceError::Other { message: "cannot view u16 output".to_string() })?;
                    (DataType::Unsigned16, view.iter().flat_map(|&x| x.to_le_bytes()).collect())
                }
            }
            DatumType::I16 => {
                if let Ok(slice) = output_value.as_slice::<i16>() {
                    (DataType::Signed16, slice_i16_to_le_bytes(slice))
                } else {
                    let view = output_value.to_array_view::<i16>().map_err(|_| InferenceError::Other { message: "cannot view i16 output".to_string() })?;
                    (DataType::Signed16, view.iter().flat_map(|&x| x.to_le_bytes()).collect())
                }
            }
            DatumType::U32 => {
                if let Ok(slice) = output_value.as_slice::<u32>() {
                    (DataType::Unsigned32, slice_u32_to_le_bytes(slice))
                } else {
                    let view = output_value.to_array_view::<u32>().map_err(|_| InferenceError::Other { message: "cannot view u32 output".to_string() })?;
                    (DataType::Unsigned32, view.iter().flat_map(|&x| x.to_le_bytes()).collect())
                }
            }
            DatumType::I32 => {
                if let Ok(slice) = output_value.as_slice::<i32>() {
                    (DataType::Signed32, slice_i32_to_le_bytes(slice))
                } else {
                    let view = output_value.to_array_view::<i32>().map_err(|_| InferenceError::Other { message: "cannot view i32 output".to_string() })?;
                    (DataType::Signed32, view.iter().flat_map(|&x| x.to_le_bytes()).collect())
                }
            }
            DatumType::U64 => {
                if let Ok(slice) = output_value.as_slice::<u64>() {
                    (DataType::Unsigned64, slice_u64_to_le_bytes(slice))
                } else {
                    let view = output_value.to_array_view::<u64>().map_err(|_| InferenceError::Other { message: "cannot view u64 output".to_string() })?;
                    (DataType::Unsigned64, view.iter().flat_map(|&x| x.to_le_bytes()).collect())
                }
            }
            DatumType::I64 => {
                if let Ok(slice) = output_value.as_slice::<i64>() {
                    (DataType::Signed64, slice_i64_to_le_bytes(slice))
                } else {
                    let view = output_value.to_array_view::<i64>().map_err(|_| InferenceError::Other { message: "cannot view i64 output".to_string() })?;
                    (DataType::Signed64, view.iter().flat_map(|&x| x.to_le_bytes()).collect())
                }
            }
            DatumType::Bool => {
                if let Ok(slice) = output_value.as_slice::<bool>() {
                    (DataType::Boolean, slice_bool_to_bytes(slice))
                } else {
                    let view = output_value.to_array_view::<bool>().map_err(|_| InferenceError::Other { message: "cannot view bool output".to_string() })?;
                    (DataType::Boolean, view.iter().map(|&b| if b { 1u8 } else { 0u8 }).collect())
                }
            }
            other => {
                return Err(InferenceError::Other { message: format!("unsupported tract output datum type: {other:?}") });
            }
        };

        TensorOutput::new(data_type, output_shape.into_boxed_slice(), None, buffer)
            .map_err(|e| InferenceError::Other { message: e.to_string() })
    }

    fn unload(&mut self) -> Result<(), InferenceError> { Ok(()) }
}

#[derive(Debug, Default)]
pub struct TractComputeBackend;

impl ComputeBackend for TractComputeBackend {
    fn load_model(&mut self, configuration: &ComputeBackendConfiguration, metadata_hint: Option<&ModelMetadata>) -> Result<Box<dyn Model>, InferenceError> {
        let model_path = configuration.parameters.get("model_path").ok_or_else(|| InferenceError::Other { message: "missing 'model_path' parameter".to_string() })?.to_string();
        let input_dtype_opt = configuration.parameters.get("input_data_type").map(|s| s.to_string());
        let input_shape_opt = configuration.parameters.get("input_shape").map(|s| s.to_string());

        let mut inference_model = tract_onnx::onnx().model_for_path(&model_path)
            .map_err(|e| InferenceError::Other { message: format!("failed to load ONNX: {e}") })?;

        if let (Some(dtype_string), Some(shape_string)) = (input_dtype_opt, input_shape_opt) {
            let dt = parse_datum_type(&dtype_string).ok_or_else(|| InferenceError::Other { message: format!("unsupported input_data_type: {dtype_string}") })?;
            let shape_usize = parse_shape_list(&shape_string).map_err(|e| InferenceError::Other { message: format!("invalid input_shape: {e}") })?;
            let dims_tdim: TVec<TDim> = shape_usize.into_iter().map(|d| d.to_dim()).collect();
            inference_model = inference_model.with_input_fact(0, InferenceFact::dt_shape(dt, dims_tdim))
                .map_err(|e| InferenceError::Other { message: format!("with_input_fact failed: {e}") })?;
        }

        let runnable = inference_model.into_optimized().and_then(|m| m.into_runnable())
            .map_err(|e| InferenceError::Other { message: format!("tract optimization/runnable failed: {e}") })?;

        let declared_metadata = metadata_hint.cloned().unwrap_or_else(ModelMetadata::empty);

        Ok(Box::new(TractOnnxModel { runnable, declared_metadata }))
    }
}

pub struct TractComputeBackendFactory;
impl ComputeBackendFactory for TractComputeBackendFactory {
    fn backend_name(&self) -> &'static str { "tract" }
    fn create_backend(&self) -> Result<Box<dyn ComputeBackend>, InferenceError> { Ok(Box::new(TractComputeBackend::default())) }
}

/* helpers */
fn parse_shape_list(s: &str) -> Result<Vec<usize>, &'static str> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() { return Err("empty dimension in input_shape"); }
        let v: usize = trimmed.parse().map_err(|_| "non-numeric dimension in input_shape")?;
        out.push(v);
    }
    if out.is_empty() { return Err("no dimensions parsed"); }
    Ok(out)
}
fn parse_datum_type(s: &str) -> Option<DatumType> {
    match s.trim().to_ascii_lowercase().as_str() {
        "f32" => Some(DatumType::F32), "f64" => Some(DatumType::F64),
        "u8" => Some(DatumType::U8),   "i8" => Some(DatumType::I8),
        "u16"=> Some(DatumType::U16),  "i16"=> Some(DatumType::I16),
        "u32"=> Some(DatumType::U32),  "i32"=> Some(DatumType::I32),
        "u64"=> Some(DatumType::U64),  "i64"=> Some(DatumType::I64),
        "bool"=> Some(DatumType::Bool),
        _ => None,
    }
}
fn bytes_to_vec_f32(b: &[u8]) -> Result<Vec<f32>, InferenceError> {
    if b.len() % 4 != 0 { return Err(InferenceError::Other { message: "f32 buffer length is not a multiple of 4".to_string() }); }
    let mut out = Vec::with_capacity(b.len()/4);
    for c in b.chunks_exact(4) { out.push(f32::from_le_bytes([c[0],c[1],c[2],c[3]])); }
    Ok(out)
}
fn bytes_to_vec_f64(b: &[u8]) -> Result<Vec<f64>, InferenceError> {
    if b.len() % 8 != 0 { return Err(InferenceError::Other { message: "f64 buffer length is not a multiple of 8".to_string() }); }
    let mut out = Vec::with_capacity(b.len()/8);
    for c in b.chunks_exact(8) { out.push(f64::from_le_bytes(c.try_into().unwrap())); }
    Ok(out)
}
fn bytes_to_vec_i8(b: &[u8]) -> Result<Vec<i8>, InferenceError> { Ok(b.iter().map(|&x| x as i8).collect()) }
fn bytes_to_vec_u16(b: &[u8]) -> Result<Vec<u16>, InferenceError> {
    if b.len() % 2 != 0 { return Err(InferenceError::Other { message: "u16 buffer length not multiple of 2".to_string() }); }
    let mut out = Vec::with_capacity(b.len()/2);
    for c in b.chunks_exact(2) { out.push(u16::from_le_bytes([c[0],c[1]])); }
    Ok(out)
}
fn bytes_to_vec_i16(b: &[u8]) -> Result<Vec<i16>, InferenceError> {
    if b.len() % 2 != 0 { return Err(InferenceError::Other { message: "i16 buffer length not multiple of 2".to_string() }); }
    let mut out = Vec::with_capacity(b.len()/2);
    for c in b.chunks_exact(2) { out.push(i16::from_le_bytes([c[0],c[1]])); }
    Ok(out)
}
fn bytes_to_vec_u32(b: &[u8]) -> Result<Vec<u32>, InferenceError> {
    if b.len() % 4 != 0 { return Err(InferenceError::Other { message: "u32 buffer length not multiple of 4".to_string() }); }
    let mut out = Vec::with_capacity(b.len()/4);
    for c in b.chunks_exact(4) { out.push(u32::from_le_bytes([c[0],c[1],c[2],c[3]])); }
    Ok(out)
}
fn bytes_to_vec_i32(b: &[u8]) -> Result<Vec<i32>, InferenceError> {
    if b.len() % 4 != 0 { return Err(InferenceError::Other { message: "i32 buffer length not multiple of 4".to_string() }); }
    let mut out = Vec::with_capacity(b.len()/4);
    for c in b.chunks_exact(4) { out.push(i32::from_le_bytes([c[0],c[1],c[2],c[3]])); }
    Ok(out)
}
fn bytes_to_vec_u64(b: &[u8]) -> Result<Vec<u64>, InferenceError> {
    if b.len() % 8 != 0 { return Err(InferenceError::Other { message: "u64 buffer length not multiple of 8".to_string() }); }
    let mut out = Vec::with_capacity(b.len()/8);
    for c in b.chunks_exact(8) { out.push(u64::from_le_bytes(c.try_into().unwrap())); }
    Ok(out)
}
fn bytes_to_vec_i64(b: &[u8]) -> Result<Vec<i64>, InferenceError> {
    if b.len() % 8 != 0 { return Err(InferenceError::Other { message: "i64 buffer length not multiple of 8".to_string() }); }
    let mut out = Vec::with_capacity(b.len()/8);
    for c in b.chunks_exact(8) { out.push(i64::from_le_bytes(c.try_into().unwrap())); }
    Ok(out)
}
fn bytes_to_vec_bool(b: &[u8]) -> Result<Vec<bool>, InferenceError> { Ok(b.iter().map(|&x| x != 0).collect()) }

fn slice_f32_to_le_bytes(s: &[f32]) -> Vec<u8> { let mut out = Vec::with_capacity(s.len()*4); for &x in s { out.extend_from_slice(&x.to_le_bytes()); } out }
fn slice_f64_to_le_bytes(s: &[f64]) -> Vec<u8> { let mut out = Vec::with_capacity(s.len()*8); for &x in s { out.extend_from_slice(&x.to_le_bytes()); } out }
fn slice_i8_to_le_bytes(s: &[i8]) -> Vec<u8> { s.iter().map(|&x| x as u8).collect() }
fn slice_u16_to_le_bytes(s: &[u16]) -> Vec<u8> { let mut out = Vec::with_capacity(s.len()*2); for &x in s { out.extend_from_slice(&x.to_le_bytes()); } out }
fn slice_i16_to_le_bytes(s: &[i16]) -> Vec<u8> { let mut out = Vec::with_capacity(s.len()*2); for &x in s { out.extend_from_slice(&x.to_le_bytes()); } out }
fn slice_u32_to_le_bytes(s: &[u32]) -> Vec<u8> { let mut out = Vec::with_capacity(s.len()*4); for &x in s { out.extend_from_slice(&x.to_le_bytes()); } out }
fn slice_i32_to_le_bytes(s: &[i32]) -> Vec<u8> { let mut out = Vec::with_capacity(s.len()*4); for &x in s { out.extend_from_slice(&x.to_le_bytes()); } out }
fn slice_u64_to_le_bytes(s: &[u64]) -> Vec<u8> { let mut out = Vec::with_capacity(s.len()*8); for &x in s { out.extend_from_slice(&x.to_le_bytes()); } out }
fn slice_i64_to_le_bytes(s: &[i64]) -> Vec<u8> { let mut out = Vec::with_capacity(s.len()*8); for &x in s { out.extend_from_slice(&x.to_le_bytes()); } out }
fn slice_bool_to_bytes(s: &[bool]) -> Vec<u8> { s.iter().map(|&b| if b { 1u8 } else { 0u8 }).collect() }
