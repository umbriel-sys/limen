#[cfg(feature = "alloc")]
use alloc::{string::String, vec::Vec};

use limen_core::types::DataType;

pub fn parse_data_type(name: &str) -> Result<DataType, String> {
    let s = name.trim();
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "boolean" => Ok(DataType::Boolean),
        "unsigned8" => Ok(DataType::Unsigned8),
        "unsigned16" => Ok(DataType::Unsigned16),
        "unsigned32" => Ok(DataType::Unsigned32),
        "unsigned64" => Ok(DataType::Unsigned64),
        "signed8" => Ok(DataType::Signed8),
        "signed16" => Ok(DataType::Signed16),
        "signed32" => Ok(DataType::Signed32),
        "signed64" => Ok(DataType::Signed64),
        "float16" => Ok(DataType::Float16),
        "bfloat16" => Ok(DataType::BFloat16),
        "float32" => Ok(DataType::Float32),
        "float64" => Ok(DataType::Float64),
        _ => Err(format!("unsupported data type name '{}'", s)),
    }
}

pub fn parse_shape_csv(csv: &str) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for part in csv.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() { return Err("empty dimension in shape".to_string()); }
        let value: usize = trimmed.parse().map_err(|_| format!("invalid dimension '{}'", trimmed))?;
        if value == 0 { return Err("shape dimensions must be greater than zero".to_string()); }
        out.push(value);
    }
    if out.is_empty() { return Err("shape must have at least one dimension".to_string()); }
    Ok(out)
}
