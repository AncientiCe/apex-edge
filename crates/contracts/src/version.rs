//! Schema versioning and compatibility.
//! Backward-compatible additive changes; breaking changes bump major.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Contract API version (semver).
///
/// # Examples
///
/// ```
/// use apex_edge_contracts::ContractVersion;
///
/// let v = ContractVersion::new(1, 2, 0);
/// assert_eq!(v.to_string(), "1.2.0");
/// assert_eq!(ContractVersion::default(), ContractVersion::V1_0_0);
/// ```
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

#[cfg(test)]
mod tests {
    use super::ContractVersion;

    #[test]
    fn default_is_v1_0_0() {
        assert_eq!(ContractVersion::default(), ContractVersion::V1_0_0);
    }

    #[test]
    fn display_formats_semver() {
        assert_eq!(ContractVersion::new(2, 4, 6).to_string(), "2.4.6");
    }
}
