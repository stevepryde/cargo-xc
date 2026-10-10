use std::ffi::OsString;
use std::fmt;
use std::str::FromStr;

use anyhow::{Context, bail};
use semver::Version;
use serde::{Deserialize, Serialize};

pub const RUST_TARGET: &str = "x86_64-unknown-linux-gnu";
pub const AARCH64_TARGET: &str = "aarch64-unknown-linux-gnu";
pub const DEFAULT_GLIBC: &str = "2.36";
pub const SUPPORTED_GLIBC: &[&str] = &["2.36"];

/// Immutable description of one provisioned toolchain family.
pub struct TargetSpec {
    pub rust_triple: &'static str,
    pub architecture: &'static str,
    pub gnu_prefix: &'static str,
    pub default_glibc: &'static str,
    pub supported_glibc: &'static [&'static str],
    pub elf_machine: u16,
    pub elf_description: &'static str,
    pub loader: &'static str,
    /// Candidate program names per tool role, most specific first.
    pub tool_candidates: &'static [(&'static str, &'static [&'static str])],
    /// Sysroot-relative libc.so.6 locations used to prove the glibc baseline.
    pub libc_candidates: &'static [&'static str],
    /// Sysroot-relative pkg-config search directories.
    pub pkg_config_dirs: &'static [&'static str],
    pub distribution_source: &'static str,
    pub distribution_release: &'static str,
    pub package_source_url: &'static str,
    pub missing_tools_hint: &'static str,
    pub builder: BuilderImage,
}

/// Docker builder identity for a target family.
pub struct BuilderImage {
    pub local_tag: &'static str,
    pub dockerfile: &'static str,
}

pub const SPECS: &[TargetSpec] = &[
    TargetSpec {
        rust_triple: RUST_TARGET,
        architecture: "x86_64",
        gnu_prefix: "x86_64-linux-gnu",
        default_glibc: DEFAULT_GLIBC,
        supported_glibc: SUPPORTED_GLIBC,
        elf_machine: goblin::elf::header::EM_X86_64,
        elf_description: "ELF64 x86-64",
        loader: "/lib64/ld-linux-x86-64.so.2",
        tool_candidates: &[
            ("cc", &["x86_64-linux-gnu-gcc"]),
            ("cxx", &["x86_64-linux-gnu-g++"]),
            ("ar", &["x86_64-linux-gnu-ar"]),
            ("ranlib", &["x86_64-linux-gnu-ranlib"]),
            ("strip", &["x86_64-linux-gnu-strip"]),
            ("objcopy", &["x86_64-linux-gnu-objcopy"]),
            ("nm", &["x86_64-linux-gnu-nm"]),
            ("readelf", &["x86_64-linux-gnu-readelf"]),
        ],
        libc_candidates: &[
            "usr/x86_64-linux-gnu/lib/libc.so.6",
            "usr/lib/x86_64-linux-gnu/libc.so.6",
            "lib/x86_64-linux-gnu/libc.so.6",
            "lib64/libc.so.6",
        ],
        pkg_config_dirs: &[
            "usr/lib/x86_64-linux-gnu/pkgconfig",
            "usr/lib/pkgconfig",
            "usr/share/pkgconfig",
        ],
        distribution_source: "Debian cross toolchain packages",
        distribution_release: "bookworm",
        package_source_url: "https://deb.debian.org/debian",
        missing_tools_hint: "On Debian ARM64 install `crossbuild-essential-amd64 g++-x86-64-linux-gnu binutils-x86-64-linux-gnu`, or use `cargo xc docker` for automatic provisioning",
        builder: BuilderImage {
            local_tag: concat!("cargo-xc:", env!("CARGO_PKG_VERSION")),
            dockerfile: "docker/Dockerfile",
        },
    },
    TargetSpec {
        rust_triple: AARCH64_TARGET,
        architecture: "aarch64",
        gnu_prefix: "aarch64-linux-gnu",
        default_glibc: "2.34",
        supported_glibc: &["2.34"],
        elf_machine: goblin::elf::header::EM_AARCH64,
        elf_description: "ELF64 AArch64",
        loader: "/lib/ld-linux-aarch64.so.1",
        tool_candidates: &[
            ("cc", &["aarch64-linux-gnu-gcc", "gcc"]),
            ("cxx", &["aarch64-linux-gnu-g++", "g++"]),
            ("ar", &["aarch64-linux-gnu-ar", "ar"]),
            ("ranlib", &["aarch64-linux-gnu-ranlib", "ranlib"]),
            ("strip", &["aarch64-linux-gnu-strip", "strip"]),
            ("objcopy", &["aarch64-linux-gnu-objcopy", "objcopy"]),
            ("nm", &["aarch64-linux-gnu-nm", "nm"]),
            ("readelf", &["aarch64-linux-gnu-readelf", "readelf"]),
        ],
        libc_candidates: &[
            "usr/lib64/libc.so.6",
            "lib64/libc.so.6",
            "usr/lib/aarch64-linux-gnu/libc.so.6",
            "lib/aarch64-linux-gnu/libc.so.6",
        ],
        pkg_config_dirs: &[
            "usr/lib64/pkgconfig",
            "usr/lib/pkgconfig",
            "usr/share/pkgconfig",
        ],
        distribution_source: "Amazon Linux 2023 native toolchain packages",
        distribution_release: "al2023",
        package_source_url: "https://cdn.amazonlinux.com/al2023",
        missing_tools_hint: "On Amazon Linux 2023 ARM64 install `gcc gcc-c++ binutils`, or use `cargo xc docker` for automatic provisioning",
        builder: BuilderImage {
            local_tag: concat!("cargo-xc:", env!("CARGO_PKG_VERSION"), "-al2023"),
            dockerfile: "docker/Dockerfile.al2023",
        },
    },
];

fn spec_for(triple: &str) -> Option<&'static TargetSpec> {
    SPECS.iter().find(|spec| spec.rust_triple == triple)
}

pub fn supported_targets() -> impl Iterator<Item = String> {
    SPECS.iter().flat_map(|spec| {
        spec.supported_glibc
            .iter()
            .map(move |baseline| format!("{}.{baseline}", spec.rust_triple))
    })
}

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
    pub fn spec(&self) -> &'static TargetSpec {
        spec_for(&self.rust_triple).expect("parsed targets always have a spec")
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
        let spec = self.spec();
        if !spec.supported_glibc.contains(&baseline.as_str()) {
            bail!(
                "glibc baseline {baseline} is not provisioned for {}; supported: {}. cargo-xc never substitutes a newer sysroot",
                spec.rust_triple,
                spec.supported_glibc.join(", ")
            );
        }
        Ok(())
    }

    #[must_use]
    pub fn lambda_default() -> Self {
        Self::from_str(AARCH64_TARGET).expect("built-in target must parse")
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
        let Some((spec, version, explicit_glibc)) = SPECS.iter().find_map(|spec| {
            if raw == spec.rust_triple {
                Some((spec, spec.default_glibc, false))
            } else {
                raw.strip_prefix(&format!("{}.", spec.rust_triple))
                    .map(|suffix| (spec, suffix, true))
            }
        }) else {
            bail!(
                "unsupported target `{raw}`; this release supports {} with an optional glibc suffix",
                SPECS
                    .iter()
                    .map(|spec| spec.rust_triple)
                    .collect::<Vec<_>>()
                    .join(" and ")
            );
        };
        let mut parts = version.split('.');
        let major = parts.next().context("missing glibc major version")?;
        let minor = parts.next().context("missing glibc minor version")?;
        if parts.next().is_some()
            || major.is_empty()
            || minor.is_empty()
            || !major.bytes().all(|b| b.is_ascii_digit())
            || !minor.bytes().all(|b| b.is_ascii_digit())
        {
            bail!(
                "malformed glibc suffix in `{raw}`; expected `{}.MAJOR.MINOR`",
                spec.rust_triple
            );
        }
        let glibc = Version::new(major.parse()?, minor.parse()?, 0);
        Ok(Self {
            rust_triple: spec.rust_triple.to_owned(),
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

/// The `--target` value in a raw argument list, if any.
pub fn find_target_argument(args: &[OsString]) -> anyhow::Result<Option<Target>> {
    for (index, argument) in args.iter().enumerate() {
        if argument == "--target" {
            return args
                .get(index + 1)
                .context("--target requires a value")?
                .to_str()
                .context("target must be UTF-8")?
                .parse()
                .map(Some);
        }
        if let Some(value) = argument
            .to_str()
            .and_then(|arg| arg.strip_prefix("--target="))
        {
            return value.parse().map(Some);
        }
    }
    Ok(None)
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
    fn parses_aarch64_target_to_al2023_default() {
        let target: Target = AARCH64_TARGET.parse().unwrap();
        assert_eq!(target.cargo_triple(), AARCH64_TARGET);
        assert_eq!(target.glibc_short(), "2.34");
        assert_eq!(target.spec().architecture, "aarch64");
        target.ensure_supported().unwrap();
    }

    #[test]
    fn rejects_unprovisioned_aarch64_baseline() {
        let target: Target = format!("{AARCH64_TARGET}.2.36").parse().unwrap();
        let error = target.ensure_supported().unwrap_err().to_string();
        assert!(error.contains("not provisioned"));
        assert!(error.contains("2.34"));
    }

    #[test]
    fn rejects_other_targets_and_bad_suffixes() {
        assert!("riscv64gc-unknown-linux-gnu".parse::<Target>().is_err());
        assert!(format!("{RUST_TARGET}.2").parse::<Target>().is_err());
        assert!(format!("{RUST_TARGET}.2.latest").parse::<Target>().is_err());
        assert!(
            format!("{AARCH64_TARGET}.2.latest")
                .parse::<Target>()
                .is_err()
        );
    }

    #[test]
    fn finds_target_argument_in_both_forms() {
        let args: Vec<OsString> = vec!["build".into(), "--target".into(), AARCH64_TARGET.into()];
        let target = find_target_argument(&args).unwrap().unwrap();
        assert_eq!(target.cargo_triple(), AARCH64_TARGET);
        let args: Vec<OsString> = vec![format!("--target={RUST_TARGET}.2.36").into()];
        let target = find_target_argument(&args).unwrap().unwrap();
        assert_eq!(target.cargo_triple(), RUST_TARGET);
        assert!(find_target_argument(&["build".into()]).unwrap().is_none());
    }
}
