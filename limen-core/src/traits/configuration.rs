#![cfg_attr(not(feature = "std"), no_std)]
#[cfg(feature = "alloc")]
use alloc::{string::String, collections::BTreeMap};

#[cfg(feature = "serde")]
use serde::{Serialize, Deserialize};

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct SensorStreamConfiguration {
    #[cfg(feature = "alloc")]
    pub label: Option<String>,
    #[cfg(feature = "alloc")]
    pub parameters: BTreeMap<String, String>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct PreprocessorConfiguration {
    #[cfg(feature = "alloc")]
    pub label: Option<String>,
    #[cfg(feature = "alloc")]
    pub parameters: BTreeMap<String, String>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct PostprocessorConfiguration {
    #[cfg(feature = "alloc")]
    pub label: Option<String>,
    #[cfg(feature = "alloc")]
    pub parameters: BTreeMap<String, String>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct OutputSinkConfiguration {
    #[cfg(feature = "alloc")]
    pub label: Option<String>,
    #[cfg(feature = "alloc")]
    pub parameters: BTreeMap<String, String>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct ComputeBackendConfiguration {
    #[cfg(feature = "alloc")]
    pub label: Option<String>,
    #[cfg(feature = "alloc")]
    pub parameters: BTreeMap<String, String>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct PlatformBackendConfiguration {
    #[cfg(feature = "alloc")]
    pub label: Option<String>,
    #[cfg(feature = "alloc")]
    pub parameters: BTreeMap<String, String>,
}
