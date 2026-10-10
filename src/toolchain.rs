use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cache::Cache;
use crate::target::Target;

const REQUIRED_TOOLS: &[(&str, &str)] = &[
    ("cc", "x86_64-linux-gnu-gcc"),
    ("cxx", "x86_64-linux-gnu-g++"),
    ("ar", "x86_64-linux-gnu-ar"),
    ("ranlib", "x86_64-linux-gnu-ranlib"),
    ("strip", "x86_64-linux-gnu-strip"),
    ("objcopy", "x86_64-linux-gnu-objcopy"),
    ("nm", "x86_64-linux-gnu-nm"),
    ("readelf", "x86_64-linux-gnu-readelf"),
];

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

    pub fn load(cache: &Cache, target: &Target) -> anyhow::Result<Self> {
        let manifest_path = cache
            .toolchain(&Self::identity(target))
            .join("manifest.json");
        let bytes = fs::read(&manifest_path).with_context(|| {
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
        toolchain.validate(target)?;
        Ok(toolchain)
    }

    pub fn install_system(cache: &Cache, target: &Target) -> anyhow::Result<Self> {
        target.ensure_supported()?;
        let _lock = cache.lock()?;
        if let Ok(existing) = Self::load(cache, target) {
            return Ok(existing);
        }

        let mut tools = BTreeMap::new();
        let mut missing = Vec::new();
        for (role, executable) in REQUIRED_TOOLS {
            match find_in_path(executable) {
                Some(path) => {
                    tools.insert((*role).to_owned(), path);
                }
                None => missing.push(*executable),
            }
        }
        if !missing.is_empty() {
            bail!(
                "native cross-toolchain programs are missing: {}. On Debian ARM64 install `crossbuild-essential-amd64 g++-x86-64-linux-gnu binutils-x86-64-linux-gnu`, or use `cargo xc docker` for automatic provisioning",
                missing.join(", ")
            );
        }
        let cc = tools.get("cc").context("discovered toolchain omitted cc")?;
        let sysroot_output = output(cc, [OsStr::new("-print-sysroot")])?;
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
        let compiler_version = output(cc, [OsStr::new("--version")])?;
        let linker_version = output(
            tools
                .get("readelf")
                .context("discovered toolchain omitted readelf")?,
            [OsStr::new("--version")],
        )?;
        validate_glibc_baseline(
            &sysroot,
            target,
            tools
                .get("readelf")
                .context("discovered toolchain omitted readelf")?,
        )?;
        let mut checksums = BTreeMap::new();
        for (role, path) in &tools {
            checksums.insert(role.clone(), sha256_file(path)?);
        }
        let manifest = ToolchainManifest {
            schema: 1,
            identity: Self::identity(target),
            target_architecture: "x86_64".into(),
            target_operating_system: "linux".into(),
            libc: "glibc".into(),
            minimum_glibc: target.glibc_short(),
            distribution_source: "Debian cross toolchain packages".into(),
            distribution_release: "bookworm".into(),
            package_versions: BTreeMap::from([
                ("cross-compiler".into(), first_line(&compiler_version)),
                ("cross-binutils".into(), first_line(&linker_version)),
            ]),
            compiler_version: first_line(&compiler_version),
            linker_version: first_line(&linker_version),
            download_urls: vec!["https://deb.debian.org/debian".into()],
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
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(&staging)?;
        fs::write(
            staging.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        fs::write(staging.join("COMPLETE"), b"cargo-xc toolchain\n")?;
        if final_dir.exists() {
            fs::remove_dir_all(&final_dir)?;
        }
        fs::rename(&staging, &final_dir)?;
        Self::load(cache, target)
    }

    pub fn validate(&self, target: &Target) -> anyhow::Result<()> {
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
            if &sha256_file(path)? != expected {
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

pub fn installed(cache: &Cache) -> anyhow::Result<Vec<ToolchainManifest>> {
    let mut manifests = Vec::new();
    let root = cache.toolchains();
    if !root.is_dir() {
        return Ok(manifests);
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path().join("manifest.json");
        if entry.path().join("COMPLETE").is_file() && path.is_file() {
            manifests.push(serde_json::from_slice(&fs::read(path)?)?);
        }
    }
    manifests.sort_by(|a, b| a.identity.cmp(&b.identity));
    Ok(manifests)
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn output<I, S>(program: &Path, args: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        bail!("{} exited with {}", program.display(), output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn first_line(value: &str) -> String {
    value.lines().next().unwrap_or_default().to_owned()
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(fs::read(path).with_context(|| format!("failed to read {}", path.display()))?);
    Ok(hex::encode(hasher.finalize()))
}

fn validate_glibc_baseline(sysroot: &Path, target: &Target, readelf: &Path) -> anyhow::Result<()> {
    let relative_candidates = [
        "usr/x86_64-linux-gnu/lib/libc.so.6",
        "usr/lib/x86_64-linux-gnu/libc.so.6",
        "lib/x86_64-linux-gnu/libc.so.6",
        "lib64/libc.so.6",
    ];
    let libc = relative_candidates
        .iter()
        .map(|relative| sysroot.join(relative))
        .find(|path| path.is_file())
        .with_context(|| {
            format!(
                "cannot locate x86_64 libc.so.6 under sysroot {}; refusing to claim glibc {}",
                sysroot.display(),
                target.glibc_short()
            )
        })?;
    let version_info = output(
        readelf,
        [
            OsStr::new("-W"),
            OsStr::new("--version-info"),
            libc.as_os_str(),
        ],
    )?;
    let actual = crate::verify::highest_glibc_version(&version_info)?
        .context("target libc contains no GLIBC version definitions")?;
    if actual.major != target.glibc.major || actual.minor != target.glibc.minor {
        bail!(
            "target sysroot {} provides glibc {}.{} but {} was requested; cargo-xc never relabels a different libc baseline",
            sysroot.display(),
            actual.major,
            actual.minor,
            target.glibc_short()
        );
    }
    Ok(())
}
