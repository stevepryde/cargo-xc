use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use tokio::fs;

use crate::process::Managed;
use crate::target::{BuilderImage, Target};

pub async fn run(args: &[OsString], trace: bool) -> anyhow::Result<u8> {
    let builder = builder_for(args)?;
    let image = ensure_image(builder, trace).await?;
    let workspace = std::env::current_dir()?;
    let cache = crate::cache::Cache::discover()?;
    cache.ensure().await?;
    let cargo_home = cache.root().join("cargo-home");
    fs::create_dir_all(&cargo_home).await.with_context(|| {
        format!(
            "failed to create isolated Docker Cargo cache {}",
            cargo_home.display()
        )
    })?;
    if is_cargo_operation(args) {
        prefetch(args, &cargo_home, trace).await?;
    }
    let mut command = Managed::new("docker");
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
        let metadata = fs::metadata(&workspace).await?;
        command
            .arg("--user")
            .arg(format!("{}:{}", metadata.uid(), metadata.gid()));
    }
    command.arg(image).arg("cargo").arg("xc").args(args);
    if trace {
        eprintln!("[cargo-xc trace] docker command: {command:?}");
    }
    let status = command.status().await.context("failed to start Docker")?;
    Ok(status.code().unwrap_or(1).try_into().unwrap_or(1))
}

/// The builder image family the forwarded invocation needs. An explicit
/// `--target` wins; `lambda` defaults to the ARM64 Lambda target.
fn builder_for(args: &[OsString]) -> anyhow::Result<&'static BuilderImage> {
    let target = match crate::target::find_target_argument(args)? {
        Some(target) => target,
        None if args.first().is_some_and(|arg| arg == "lambda") => Target::lambda_default(),
        None => Target::default(),
    };
    Ok(&target.spec().builder)
}

// Mirrors the management commands routed by `cli::run`; everything else builds with Cargo.
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
async fn prefetch(args: &[OsString], cargo_home: &Path, trace: bool) -> anyhow::Result<()> {
    if args
        .iter()
        .any(|arg| arg == "--offline" || arg == "--frozen")
    {
        return Ok(());
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Managed::new(cargo);
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
        .await
        .context("failed to run cargo fetch on the host")?;
    if !status.success() {
        bail!(
            "cargo fetch failed on the host; private Git dependencies use your host Git credentials"
        );
    }
    Ok(())
}

async fn ensure_image(builder: &BuilderImage, trace: bool) -> anyhow::Result<String> {
    let mut inspect = Managed::new("docker");
    inspect.args(["image", "inspect", builder.local_tag]);
    let exists = inspect
        .output()
        .await
        .is_ok_and(|output| output.status.success());
    if exists {
        return Ok(builder.local_tag.into());
    }
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut command = Managed::new("docker");
    command
        .args([
            "build",
            "--platform",
            "linux/arm64",
            "--tag",
            builder.local_tag,
            "--file",
        ])
        .arg(repo.join(builder.dockerfile))
        .arg(&repo);
    if trace {
        eprintln!("[cargo-xc trace] docker build: {command:?}");
    }
    let status = command
        .status()
        .await
        .context("failed to build ARM64 cargo-xc image")?;
    if !status.success() {
        bail!("Docker failed to build {}", builder.local_tag);
    }
    Ok(builder.local_tag.into())
}
