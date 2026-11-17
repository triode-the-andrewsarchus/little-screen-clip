//! Explicit catalog overrides inherit only omitted fields from daemon settings.
use super::DriveConfig;
use serde::{Deserialize, Serialize};

/// Effective metadata-capture budgets, independent of global preferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogLimits {
    pub entries: u64,
    pub bytes: u64,
    pub depth: usize,
    pub timeout_seconds: u64,
}
impl From<&DriveConfig> for CatalogLimits {
    fn from(config: &DriveConfig) -> Self {
        Self {
            entries: config.catalog_max_entries,
            bytes: config.catalog_max_bytes,
            depth: config.catalog_max_depth,
            timeout_seconds: config.catalog_timeout_seconds,
        }
    }
}
impl CatalogLimits {
    /// Validate budgets before creating a catalog database.
    ///
    /// # Errors
    /// Returns a description of an unusable SQLite byte, traversal or time limit.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.bytes < 128 * 1024 || self.bytes > i64::MAX as u64 {
            return Err("Catalog database bytes must be between 131072 and 9223372036854775807");
        }
        if self.depth == 0 || self.timeout_seconds == 0 {
            return Err("Catalogs require nonzero depth and time limits");
        }
        Ok(())
    }

    /// Apply only catalog fields to an operation's configuration snapshot.
    pub fn apply_to(&self, config: &mut DriveConfig) {
        config.catalog_max_entries = self.entries;
        config.catalog_max_bytes = self.bytes;
        config.catalog_max_depth = self.depth;
        config.catalog_timeout_seconds = self.timeout_seconds;
    }
}

/// Optional per-capture fields; omitted values use the daemon's current settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CatalogLimitOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}
impl CatalogLimitOverrides {
    /// Resolve an explicit request against the worker's captured default budgets.
    pub fn resolve(&self, defaults: CatalogLimits) -> CatalogLimits {
        CatalogLimits {
            entries: self.entries.unwrap_or(defaults.entries),
            bytes: self.bytes.unwrap_or(defaults.bytes),
            depth: self.depth.unwrap_or(defaults.depth),
            timeout_seconds: self.timeout_seconds.unwrap_or(defaults.timeout_seconds),
        }
    }
}
impl From<CatalogLimits> for CatalogLimitOverrides {
    fn from(limits: CatalogLimits) -> Self {
        Self {
            entries: Some(limits.entries),
            bytes: Some(limits.bytes),
            depth: Some(limits.depth),
            timeout_seconds: Some(limits.timeout_seconds),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drives::DriveRequest;

    #[test]
    fn partial_overrides_inherit_custom_daemon_limits_and_change_only_catalog_fields() {
        let mut config = DriveConfig {
            catalog_max_entries: 12,
            catalog_max_bytes: 8 * 1024 * 1024,
            catalog_max_depth: 7,
            catalog_timeout_seconds: 91,
            ram_min_available: 123,
            catalog_workers: 3,
            backup_directory: Some("/owned/backups".into()),
            ..Default::default()
        };
        let limits = CatalogLimitOverrides {
            entries: Some(1_000_000),
            ..Default::default()
        }
        .resolve((&config).into());
        assert_eq!(
            limits,
            CatalogLimits {
                entries: 1_000_000,
                bytes: 8 * 1024 * 1024,
                depth: 7,
                timeout_seconds: 91
            }
        );
        limits.validate().unwrap();
        limits.apply_to(&mut config);
        assert_eq!(CatalogLimits::from(&config), limits);
        assert_eq!(config.ram_min_available, 123);
        assert_eq!(config.catalog_workers, 3);
        assert_eq!(
            config.backup_directory.unwrap(),
            std::path::Path::new("/owned/backups")
        );
    }

    #[test]
    fn old_preferences_default_to_one_worker_and_explicit_counts_roundtrip() {
        let old: DriveConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(old.catalog_workers, 1);
        let configured = DriveConfig {
            catalog_workers: crate::drives::MAX_CATALOG_WORKERS,
            ..old
        };
        let decoded: DriveConfig =
            serde_json::from_slice(&serde_json::to_vec(&configured).unwrap()).unwrap();
        assert_eq!(decoded.catalog_workers, crate::drives::MAX_CATALOG_WORKERS);
    }

    #[test]
    fn explicit_limits_fail_closed_on_old_daemons_and_legacy_fingerprints_stay_unchanged() {
        #[derive(Serialize, Deserialize)]
        enum LegacyRequest {
            Catalog { drive: String },
        }
        let old = LegacyRequest::Catalog {
            drive: "owned".into(),
        };
        let wire = serde_json::to_vec(&old).unwrap();
        let current: DriveRequest = serde_json::from_slice(&wire).unwrap();
        assert_eq!(serde_json::to_vec(&current).unwrap(), wire);
        let new = DriveRequest::CatalogWithLimits {
            drive: "owned".into(),
            limits: CatalogLimitOverrides {
                entries: Some(1_000_000),
                ..Default::default()
            },
        };
        let encoded = serde_json::to_vec(&new).unwrap();
        assert!(serde_json::from_slice::<LegacyRequest>(&encoded).is_err());
        let decoded: DriveRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), encoded);
        assert!(serde_json::from_str::<CatalogLimitOverrides>(r#"{"max_entries":1000}"#).is_err());
    }

    #[test]
    fn invalid_catalog_budgets_are_rejected_but_zero_entries_is_an_explicit_bound() {
        let good = CatalogLimits::from(&DriveConfig::default());
        for invalid in [
            CatalogLimits { bytes: 0, ..good },
            CatalogLimits {
                bytes: u64::MAX,
                ..good
            },
            CatalogLimits { depth: 0, ..good },
            CatalogLimits {
                timeout_seconds: 0,
                ..good
            },
        ] {
            assert!(invalid.validate().is_err());
        }
        CatalogLimits { entries: 0, ..good }.validate().unwrap();
        assert_eq!(
            CatalogLimitOverrides::from(good).resolve(CatalogLimits { entries: 0, ..good }),
            good
        );
    }
}
