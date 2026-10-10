use std::fmt;
use std::str::FromStr;

use anyhow::{Context, bail};
use semver::Version;
use serde::{Deserialize, Serialize};

pub const RUST_TARGET: &str = "x86_64-unknown-linux-gnu";
pub const DEFAULT_GLIBC: &str = "2.36";
pub const SUPPORTED_GLIBC: &[&str] = &["2.36"];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub rust_triple: String,
    pub glibc: Version,
    pub explicit_glibc: bool,
}

impl Target {
    #[must_use]
    pub fn cargo_triple(&self) -> &str {
        &self.rust_triple
    }

    #[must_use]
    pub fn glibc_short(&self) -> String {
        format!("{}.{}", self.glibc.major, self.glibc.minor)
    }

    #[must_use]
    pub fn qualified(&self) -> String {
        format!("{}.{}", self.rust_triple, self.glibc_short())
    }

    #[must_use]
    pub fn env_key(&self) -> String {
        self.rust_triple.replace('-', "_")
    }

    #[must_use]
    pub fn cargo_env_key(&self) -> String {
        self.env_key().to_ascii_uppercase()
    }

    pub fn ensure_supported(&self) -> anyhow::Result<()> {
        let baseline = self.glibc_short();
        if !SUPPORTED_GLIBC.contains(&baseline.as_str()) {
            bail!(
                "glibc baseline {baseline} is not provisioned by this release; supported: {}. cargo-xc never substitutes a newer sysroot",
                SUPPORTED_GLIBC.join(", ")
            );
        }
        Ok(())
    }
}

impl Default for Target {
    fn default() -> Self {
        Self::from_str(RUST_TARGET).expect("built-in target must parse")
    }
}

impl FromStr for Target {
    type Err = anyhow::Error;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (triple, version, explicit_glibc) =
            if let Some(suffix) = raw.strip_prefix(&format!("{RUST_TARGET}.")) {
                (RUST_TARGET, suffix, true)
            } else {
                (raw, DEFAULT_GLIBC, false)
            };
        if triple != RUST_TARGET {
            bail!(
                "unsupported target `{raw}`; this release supports only {RUST_TARGET} with an optional glibc suffix"
            );
        }
        let mut parts = version.split('.');
        let major = parts.next().context("missing glibc major version")?;
        let minor = parts.next().context("missing glibc minor version")?;
        if parts.next().is_some()
            || major.is_empty()
            || minor.is_empty()
            || !major.bytes().all(|b| b.is_ascii_digit())
            || !minor.bytes().all(|b| b.is_ascii_digit())
        {
            bail!("malformed glibc suffix in `{raw}`; expected `{RUST_TARGET}.MAJOR.MINOR`");
        }
        let glibc = Version::new(major.parse()?, minor.parse()?, 0);
        Ok(Self {
            rust_triple: triple.to_owned(),
            glibc,
            explicit_glibc,
        })
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.qualified())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_target_to_pinned_default() {
        let target: Target = RUST_TARGET.parse().unwrap();
        assert_eq!(target.glibc_short(), "2.36");
        assert!(!target.explicit_glibc);
    }

    #[test]
    fn parses_versioned_target() {
        let target: Target = format!("{RUST_TARGET}.2.17").parse().unwrap();
        assert_eq!(target.cargo_triple(), RUST_TARGET);
        assert_eq!(target.glibc_short(), "2.17");
        assert!(target.explicit_glibc);
    }

    #[test]
    fn rejects_other_targets_and_bad_suffixes() {
        assert!("aarch64-unknown-linux-gnu".parse::<Target>().is_err());
        assert!(format!("{RUST_TARGET}.2").parse::<Target>().is_err());
        assert!(format!("{RUST_TARGET}.2.latest").parse::<Target>().is_err());
    }
}
