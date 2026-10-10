use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::process::Managed;
use crate::target::Target;
use crate::toolchain::Toolchain;

pub const LOCK_FILE: &str = "cargo-xc.lock";
pub const CONFIG_FILE: &str = "cargo-xc.toml";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XcLock {
    pub schema: u32,
    pub cargo_xc_version: String,
    pub target: String,
    pub rustc: String,
    pub cargo: String,
    pub toolchain_identity: String,
    pub compiler_version: String,
    pub linker_version: String,
    pub sysroot: PathBuf,
    pub tool_checksums: BTreeMap<String, String>,
    pub docker_image: Option<String>,
}

pub async fn init(directory: &Path) -> anyhow::Result<PathBuf> {
    let path = directory.join(CONFIG_FILE);
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    fs::write(
        &path,
        "# cargo-xc project configuration\n[toolchain]\ntarget = \"x86_64-unknown-linux-gnu.2.36\"\n\n# Target-library overlays are planned but not enabled in v0.1.\n",
    )
    .await?;
    Ok(path)
}

pub async fn write(
    directory: &Path,
    target: &Target,
    toolchain: &Toolchain,
    update: bool,
) -> anyhow::Result<PathBuf> {
    let path = directory.join(LOCK_FILE);
    if path.exists() && !update {
        bail!(
            "{} already exists; pass --update to replace it",
            path.display()
        );
    }
    let lock = XcLock {
        schema: 1,
        cargo_xc_version: crate::VERSION.into(),
        target: target.qualified(),
        rustc: version("rustc").await?,
        cargo: version("cargo").await?,
        toolchain_identity: toolchain.manifest.identity.clone(),
        compiler_version: toolchain.manifest.compiler_version.clone(),
        linker_version: toolchain.manifest.linker_version.clone(),
        sysroot: toolchain.manifest.sysroot.clone(),
        tool_checksums: toolchain.manifest.checksums.clone(),
        docker_image: None,
    };
    fs::write(&path, toml::to_string_pretty(&lock)?).await?;
    Ok(path)
}

pub async fn read(directory: &Path) -> anyhow::Result<XcLock> {
    let path = directory.join(LOCK_FILE);
    toml::from_str(
        &fs::read_to_string(&path)
            .await
            .with_context(|| format!("{} is missing; run `cargo xc lock`", path.display()))?,
    )
    .with_context(|| format!("invalid {}", path.display()))
}

pub async fn verify(directory: &Path, toolchain: &Toolchain) -> anyhow::Result<()> {
    let lock = read(directory).await?;
    if lock.toolchain_identity != toolchain.manifest.identity
        || lock.tool_checksums != toolchain.manifest.checksums
        || lock.sysroot != toolchain.manifest.sysroot
    {
        bail!(
            "{LOCK_FILE} does not match the installed toolchain; run `cargo xc lock --update` only after reviewing the toolchain change"
        );
    }
    Ok(())
}

async fn version(program: &str) -> anyhow::Result<String> {
    let mut command = Managed::new(program);
    command.arg("--version");
    let output = command.output().await?;
    if !output.status.success() {
        bail!("failed to query {program} version");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
