use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

const LOCAL_IMAGE: &str = "cargo-xc:local";
const RELEASE_IMAGE: &str = concat!("ghcr.io/stevepryde/cargo-xc:", env!("CARGO_PKG_VERSION"));

pub fn run(args: &[OsString], trace: bool) -> anyhow::Result<u8> {
    let image = ensure_image(trace)?;
    let workspace = std::env::current_dir()?;
    let cache = crate::cache::Cache::discover()?;
    cache.ensure()?;
    let cargo_home = cache.root().join("cargo-home");
    std::fs::create_dir_all(&cargo_home).with_context(|| {
        format!(
            "failed to create isolated Docker Cargo cache {}",
            cargo_home.display()
        )
    })?;
    if is_cargo_operation(args) {
        prefetch(args, &cargo_home, trace)?;
    }
    let mut command = Command::new("docker");
    command
        .args(["run", "--rm", "--platform", "linux/arm64"])
        .arg("-v")
        .arg(format!("{}:/workspace", workspace.display()))
        .arg("-v")
        .arg(format!("{}:/cargo-cache", cargo_home.display()))
        .arg("-v")
        .arg(format!("{}:/xc-cache", cache.root().display()))
        .args(["-e", "CARGO_HOME=/cargo-cache"])
        .args(["-e", "CARGO_XC_CACHE_DIR=/xc-cache"])
        .args(["-w", "/workspace"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(&workspace)?;
        command
            .arg("--user")
            .arg(format!("{}:{}", metadata.uid(), metadata.gid()));
    }
    command.arg(image).arg("cargo").arg("xc").args(args);
    if trace {
        eprintln!("[cargo-xc trace] docker command: {command:?}");
    }
    let status = command.status().context("failed to start Docker")?;
    Ok(status.code().unwrap_or(1).try_into().unwrap_or(1))
}

// Mirrors the management commands routed by `cli::run`; everything else is a Cargo operation.
fn is_cargo_operation(args: &[OsString]) -> bool {
    !matches!(
        args.first().and_then(|arg| arg.to_str()),
        None | Some(
            "--help"
                | "-h"
                | "help"
                | "--version"
                | "-V"
                | "targets"
                | "docker"
                | "doctor"
                | "init"
                | "lock"
                | "verify"
                | "toolchain"
                | "cache"
        )
    )
}

/// Downloads dependencies on the host with the user's own Git credentials so the
/// container never receives any credential.
fn prefetch(args: &[OsString], cargo_home: &Path, trace: bool) -> anyhow::Result<()> {
    if args
        .iter()
        .any(|arg| arg == "--offline" || arg == "--frozen")
    {
        return Ok(());
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .arg("fetch")
        .env("CARGO_HOME", cargo_home)
        .env("CARGO_NET_GIT_FETCH_WITH_CLI", "true");
    for (index, arg) in args.iter().enumerate() {
        if arg == "--locked" {
            command.arg(arg);
        } else if arg == "--manifest-path" {
            command.arg(arg).arg(
                args.get(index + 1)
                    .context("--manifest-path requires a value")?,
            );
        } else if arg
            .to_str()
            .is_some_and(|value| value.starts_with("--manifest-path="))
        {
            command.arg(arg);
        }
    }
    if trace {
        eprintln!("[cargo-xc trace] host prefetch: {command:?}");
    }
    let status = command
        .status()
        .context("failed to run cargo fetch on the host")?;
    if !status.success() {
        bail!(
            "cargo fetch failed on the host; private Git dependencies use your host Git credentials"
        );
    }
    Ok(())
}

fn ensure_image(trace: bool) -> anyhow::Result<String> {
    let exists = Command::new("docker")
        .args(["image", "inspect", LOCAL_IMAGE])
        .output()
        .is_ok_and(|output| output.status.success());
    if exists {
        return Ok(LOCAL_IMAGE.into());
    }
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dockerfile = repo.join("docker/Dockerfile");
    if !dockerfile.is_file() {
        let status = Command::new("docker")
            .args(["pull", "--platform", "linux/arm64", RELEASE_IMAGE])
            .status()
            .context("failed to pull the released ARM64 cargo-xc image")?;
        if !status.success() {
            bail!(
                "could not pull {RELEASE_IMAGE}; the installed cargo-xc version needs its matching released builder image"
            );
        }
        return Ok(RELEASE_IMAGE.into());
    }
    let mut command = Command::new("docker");
    command
        .args([
            "build",
            "--platform",
            "linux/arm64",
            "--tag",
            LOCAL_IMAGE,
            "--file",
        ])
        .arg(dockerfile)
        .arg(&repo);
    if trace {
        eprintln!("[cargo-xc trace] docker build: {command:?}");
    }
    let status = command
        .status()
        .context("failed to build ARM64 cargo-xc image")?;
    if !status.success() {
        bail!("Docker failed to build {LOCAL_IMAGE}");
    }
    Ok(LOCAL_IMAGE.into())
}
