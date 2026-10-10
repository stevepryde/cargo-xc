use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;

use crate::cache::Cache;
use crate::process::Managed;
use crate::target::Target;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolchainManifest {
    pub schema: u32,
    pub identity: String,
    pub target_architecture: String,
    pub target_operating_system: String,
    pub libc: String,
    pub minimum_glibc: String,
    pub distribution_source: String,
    pub distribution_release: String,
    pub package_versions: BTreeMap<String, String>,
    pub compiler_version: String,
    pub linker_version: String,
    pub download_urls: Vec<String>,
    pub checksums: BTreeMap<String, String>,
    pub creation_timestamp_unix: u64,
    pub tool_version: String,
    pub sysroot: PathBuf,
    pub tools: BTreeMap<String, PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Toolchain {
    pub manifest: ToolchainManifest,
    pub manifest_path: PathBuf,
}

impl Toolchain {
    #[must_use]
    pub fn identity(target: &Target) -> String {
        target.qualified()
    }

    pub async fn load(cache: &Cache, target: &Target) -> anyhow::Result<Self> {
        let manifest_path = cache
            .toolchain(&Self::identity(target))
            .join("manifest.json");
        let bytes = fs::read(&manifest_path).await.with_context(|| {
            format!(
                "toolchain {} is not installed; run `cargo xc toolchain install {target}` or use `cargo xc docker`",
                target.qualified()
            )
        })?;
        let manifest: ToolchainManifest = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid toolchain manifest {}", manifest_path.display()))?;
        let toolchain = Self {
            manifest,
            manifest_path,
        };
        toolchain.validate(target).await?;
        Ok(toolchain)
    }

    pub async fn install_system(cache: &Cache, target: &Target) -> anyhow::Result<Self> {
        target.ensure_supported()?;
        let spec = target.spec();
        let _lock = cache.lock().await?;
        if let Ok(existing) = Self::load(cache, target).await {
            return Ok(existing);
        }

        let tools = discover_tools(spec)?;
        let cc = tools.get("cc").context("discovered toolchain omitted cc")?;
        let sysroot_output = output(cc, [OsStr::new("-print-sysroot")]).await?;
        let sysroot = if sysroot_output.trim().is_empty() {
            PathBuf::from("/")
        } else {
            PathBuf::from(sysroot_output.trim())
        };
        if !sysroot.exists() {
            bail!(
                "cross compiler reported missing sysroot {}",
                sysroot.display()
            );
        }
        let compiler_version = output(cc, [OsStr::new("--version")]).await?;
        let linker_version = output(
            tools
                .get("readelf")
                .context("discovered toolchain omitted readelf")?,
            [OsStr::new("--version")],
        )
        .await?;
        validate_glibc_baseline(&sysroot, target).await?;
        let mut checksums = BTreeMap::new();
        for (role, path) in &tools {
            checksums.insert(role.clone(), sha256_file(path).await?);
        }
        let manifest = ToolchainManifest {
            schema: 1,
            identity: Self::identity(target),
            target_architecture: spec.architecture.into(),
            target_operating_system: "linux".into(),
            libc: "glibc".into(),
            minimum_glibc: target.glibc_short(),
            distribution_source: spec.distribution_source.into(),
            distribution_release: spec.distribution_release.into(),
            package_versions: BTreeMap::from([
                ("cross-compiler".into(), first_line(&compiler_version)),
                ("cross-binutils".into(), first_line(&linker_version)),
            ]),
            compiler_version: first_line(&compiler_version),
            linker_version: first_line(&linker_version),
            download_urls: vec![spec.package_source_url.into()],
            checksums,
            creation_timestamp_unix: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            tool_version: crate::VERSION.into(),
            sysroot,
            tools,
        };
        let final_dir = cache.toolchain(&manifest.identity);
        let staging = cache
            .toolchains()
            .join(format!(".{}.installing", manifest.identity));
        if staging.exists() {
            fs::remove_dir_all(&staging).await?;
        }
        fs::create_dir_all(&staging).await?;
        fs::write(
            staging.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )
        .await?;
        fs::write(staging.join("COMPLETE"), b"cargo-xc toolchain\n").await?;
        if final_dir.exists() {
            fs::remove_dir_all(&final_dir).await?;
        }
        fs::rename(&staging, &final_dir).await?;
        Self::load(cache, target).await
    }

    pub async fn validate(&self, target: &Target) -> anyhow::Result<()> {
        let parent = self
            .manifest_path
            .parent()
            .context("manifest has no parent")?;
        if !parent.join("COMPLETE").is_file() {
            bail!("toolchain install is incomplete: {}", parent.display());
        }
        if self.manifest.identity != Self::identity(target) {
            bail!("toolchain manifest identity does not match requested target");
        }
        for (role, path) in &self.manifest.tools {
            if !path.is_file() {
                bail!("toolchain `{role}` program is missing: {}", path.display());
            }
            let expected = self
                .manifest
                .checksums
                .get(role)
                .context("missing tool checksum")?;
            if &sha256_file(path).await? != expected {
                bail!("toolchain `{role}` checksum changed at {}", path.display());
            }
        }
        Ok(())
    }

    pub fn tool(&self, role: &str) -> anyhow::Result<&Path> {
        self.manifest
            .tools
            .get(role)
            .map(PathBuf::as_path)
            .with_context(|| format!("toolchain manifest does not define the `{role}` program"))
    }
}

pub async fn installed(cache: &Cache) -> anyhow::Result<Vec<ToolchainManifest>> {
    let mut manifests = Vec::new();
    let root = cache.toolchains();
    if !root.is_dir() {
        return Ok(manifests);
    }
    let mut entries = fs::read_dir(root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path().join("manifest.json");
        if entry.path().join("COMPLETE").is_file() && path.is_file() {
            manifests.push(serde_json::from_slice(&fs::read(path).await?)?);
        }
    }
    manifests.sort_by(|a, b| a.identity.cmp(&b.identity));
    Ok(manifests)
}

fn discover_tools(spec: &crate::target::TargetSpec) -> anyhow::Result<BTreeMap<String, PathBuf>> {
    let mut tools = BTreeMap::new();
    let mut missing = Vec::new();
    for (role, candidates) in spec.tool_candidates {
        match candidates.iter().find_map(|name| find_in_path(name)) {
            Some(path) => {
                tools.insert((*role).to_owned(), path);
            }
            None => missing.push(candidates.join("/")),
        }
    }
    if missing.is_empty() {
        Ok(tools)
    } else {
        bail!(
            "native toolchain programs for {} are missing: {}. {}",
            spec.rust_triple,
            missing.join(", "),
            spec.missing_tools_hint
        );
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

async fn output<I, S>(program: &Path, args: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Managed::new(program);
    command.args(args);
    let output = command.output().await?;
    if !output.status.success() {
        bail!("{} exited with {}", program.display(), output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn first_line(value: &str) -> String {
    value.lines().next().unwrap_or_default().to_owned()
}

async fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(
        fs::read(path)
            .await
            .with_context(|| format!("failed to read {}", path.display()))?,
    );
    Ok(hex::encode(hasher.finalize()))
}

/// Prove the sysroot libc is exactly the requested release baseline, read
/// from the libc's embedded `release version MAJOR.MINOR` banner.
async fn validate_glibc_baseline(sysroot: &Path, target: &Target) -> anyhow::Result<()> {
    let libc = target
        .spec()
        .libc_candidates
        .iter()
        .map(|relative| sysroot.join(relative))
        .find(|path| path.is_file())
        .with_context(|| {
            format!(
                "cannot locate {} libc.so.6 under sysroot {}; refusing to claim glibc {}",
                target.spec().architecture,
                sysroot.display(),
                target.glibc_short()
            )
        })?;
    let bytes = fs::read(&libc)
        .await
        .with_context(|| format!("failed to read {}", libc.display()))?;
    let banner = regex::bytes::Regex::new(r"release version (\d+)\.(\d+)")?;
    let release = banner
        .captures(&bytes)
        .map(|captures| -> anyhow::Result<Version> {
            Ok(Version::new(
                std::str::from_utf8(&captures[1])?.parse()?,
                std::str::from_utf8(&captures[2])?.parse()?,
                0,
            ))
        })
        .transpose()?
        .with_context(|| {
            format!(
                "{} carries no glibc release banner; refusing to claim glibc {}",
                libc.display(),
                target.glibc_short()
            )
        })?;
    if release.major != target.glibc.major || release.minor != target.glibc.minor {
        bail!(
            "target sysroot {} provides glibc {}.{} but {} was requested; cargo-xc never relabels a different libc baseline",
            sysroot.display(),
            release.major,
            release.minor,
            target.glibc_short()
        );
    }
    Ok(())
}
