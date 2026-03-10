//! Schema versioning and compatibility.
//! Backward-compatible additive changes; breaking changes bump major.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Contract API version (semver).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ContractVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl ContractVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub const V1_0_0: Self = Self::new(1, 0, 0);
}

impl fmt::Display for ContractVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Default for ContractVersion {
    fn default() -> Self {
        Self::V1_0_0
    }
}
