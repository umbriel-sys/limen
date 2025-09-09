#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
use alloc::string::String;
use core::fmt;

#[derive(Debug)]
pub enum SensorError {
    OpenFailed,
    ReadFailed,
    EndOfStream,
    ResetFailed,
    ConfigurationInvalid,
    Other {
        #[cfg(feature = "alloc")]
        message: String,
    },
}

impl fmt::Display for SensorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SensorError::OpenFailed => write!(f, "sensor open failed"),
            SensorError::ReadFailed => write!(f, "sensor read failed"),
            SensorError::EndOfStream => write!(f, "end of stream"),
            SensorError::ResetFailed => write!(f, "sensor reset failed"),
            SensorError::ConfigurationInvalid => write!(f, "sensor configuration invalid"),
            SensorError::Other { .. } => write!(f, "sensor other error"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for SensorError {}

#[derive(Debug)]
pub enum ProcessingError {
    InvalidData {
        #[cfg(feature = "alloc")]
        message: String,
    },
    OperationFailed {
        #[cfg(feature = "alloc")]
        message: String,
    },
}

impl fmt::Display for ProcessingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessingError::InvalidData { .. } => write!(f, "invalid data"),
            ProcessingError::OperationFailed { .. } => write!(f, "operation failed"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ProcessingError {}

#[derive(Debug)]
pub enum InferenceError {
    BackendUnavailable,
    Other {
        #[cfg(feature = "alloc")]
        message: String,
    },
}

impl fmt::Display for InferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InferenceError::BackendUnavailable => write!(f, "backend unavailable"),
            InferenceError::Other { .. } => write!(f, "inference error"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for InferenceError {}

#[derive(Debug)]
pub enum OutputError {
    WriteFailed,
    FlushFailed,
    Other {
        #[cfg(feature = "alloc")]
        message: String,
    },
}

impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputError::WriteFailed => write!(f, "write failed"),
            OutputError::FlushFailed => write!(f, "flush failed"),
            OutputError::Other { .. } => write!(f, "output error"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for OutputError {}

#[derive(Debug)]
pub enum RuntimeError {
    RuntimeNotOpen,
    ComponentFailureDuringOpen,
    ComponentFailureDuringClose,
    StepFailed,
    Other {
        #[cfg(feature = "alloc")]
        message: String,
    },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::RuntimeNotOpen => write!(f, "runtime is not open"),
            RuntimeError::ComponentFailureDuringOpen => write!(f, "component failure during open"),
            RuntimeError::ComponentFailureDuringClose => {
                write!(f, "component failure during close")
            }
            RuntimeError::StepFailed => write!(f, "step failed"),
            RuntimeError::Other { .. } => write!(f, "runtime error"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RuntimeError {}
