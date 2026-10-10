use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{Context, bail};

use crate::cache::Cache;
use crate::target::Target;
use crate::toolchain::Toolchain;
use crate::verify::{print_verification, verify_recent_artifacts};
use crate::wrappers::{Wrappers, prepend_path};

const OPERATIONS: &[&str] = &["build", "check", "rustc", "clippy", "doc"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CargoInvocation {
    pub operation: String,
    pub args: Vec<OsString>,
    pub target: Target,
}

impl CargoInvocation {
    pub fn parse(raw: impl IntoIterator<Item = OsString>) -> anyhow::Result<Self> {
        let mut args: Vec<OsString> = raw.into_iter().collect();
        let explicit_operation = args
            .first()
            .and_then(|value| value.to_str())
            .filter(|value| OPERATIONS.contains(value))
            .map(ToOwned::to_owned);
        let operation = if let Some(operation) = explicit_operation {
            args.remove(0);
            operation
        } else {
            "build".to_owned()
        };
        let mut target = None;
        let mut index = 0;
        while index < args.len() {
            if args[index] == "--target" {
                let raw_target = args.get(index + 1).context("--target requires a value")?;
                let parsed: Target = raw_target
                    .to_str()
                    .context("target must be valid UTF-8")?
                    .parse()?;
                args[index + 1] = parsed.cargo_triple().into();
                target = Some(parsed);
                index += 2;
                continue;
            }
            if let Some(value) = args[index]
                .to_str()
                .and_then(|arg| arg.strip_prefix("--target="))
            {
                let parsed: Target = value.parse()?;
                args[index] = format!("--target={}", parsed.cargo_triple()).into();
                target = Some(parsed);
            }
            index += 1;
        }
        let target = target
            .context("a target is required; use `--target x86_64-unknown-linux-gnu[.GLIBC]`")?;
        target.ensure_supported()?;
        Ok(Self {
            operation,
            args,
            target,
        })
    }
}

pub fn execute(
    invocation: &CargoInvocation,
    cache: &Cache,
    offline: bool,
    trace: bool,
) -> anyhow::Result<u8> {
    ensure_rust_target(invocation.target.cargo_triple(), offline, trace)?;
    let toolchain = Toolchain::load(cache, &invocation.target).or_else(|error| {
        if offline {
            Err(error)
        } else {
            Toolchain::install_system(cache, &invocation.target)
        }
    })?;
    let wrappers = Wrappers::prepare(cache, &invocation.target, &toolchain)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(&cargo);
    command.arg(&invocation.operation).args(&invocation.args);
    configure_environment(&mut command, invocation, &toolchain, &wrappers)?;
    if trace {
        eprintln!("[cargo-xc trace] cargo command: {command:?}");
    }
    let started = SystemTime::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let status = command.status().context("failed to start Cargo")?;
    if !status.success() {
        return Ok(status.code().unwrap_or(1).try_into().unwrap_or(1));
    }

    if should_verify(&invocation.operation) {
        let output_root = artifact_root(&cargo, invocation)?;
        let results = verify_recent_artifacts(
            &output_root,
            started,
            &invocation.target,
            wrappers.program("readelf")?,
        )?;
        for result in &results {
            print_verification(result, &invocation.target);
        }
        if results.is_empty()
            && !matches!(invocation.operation.as_str(), "check" | "clippy" | "doc")
        {
            eprintln!(
                "warning: cargo-xc found no newly written final ELF artifacts under {}; use `cargo xc verify <path>` for an explicit check",
                output_root.display()
            );
        }
    }
    Ok(0)
}

fn configure_environment(
    command: &mut Command,
    invocation: &CargoInvocation,
    toolchain: &Toolchain,
    wrappers: &Wrappers,
) -> anyhow::Result<()> {
    let underscored = invocation.target.env_key();
    let dashed = invocation.target.cargo_triple();
    let cargo_key = invocation.target.cargo_env_key();
    for suffix in [underscored.as_str(), dashed] {
        command.env(format!("CC_{suffix}"), wrappers.program("cc")?);
        command.env(format!("CXX_{suffix}"), wrappers.program("cxx")?);
        command.env(format!("AR_{suffix}"), wrappers.program("ar")?);
        command.env(format!("RANLIB_{suffix}"), wrappers.program("ranlib")?);
        command.env(
            format!("CFLAGS_{suffix}"),
            "--sysroot".to_owned() + "=" + &toolchain.manifest.sysroot.display().to_string(),
        );
        command.env(
            format!("CXXFLAGS_{suffix}"),
            "--sysroot".to_owned() + "=" + &toolchain.manifest.sysroot.display().to_string(),
        );
    }
    command
        .env(
            format!("CARGO_TARGET_{cargo_key}_LINKER"),
            wrappers.program("cc")?,
        )
        .env("TARGET_CC", wrappers.program("cc")?)
        .env("TARGET_CXX", wrappers.program("cxx")?)
        .env("TARGET_AR", wrappers.program("ar")?)
        .env("TARGET_RANLIB", wrappers.program("ranlib")?)
        .env("TARGET_PKG_CONFIG", wrappers.program("pkg_config")?)
        .env("PKG_CONFIG", wrappers.program("pkg_config")?)
        .env("PKG_CONFIG_ALLOW_CROSS", "1")
        .env("PKG_CONFIG_SYSROOT_DIR", &toolchain.manifest.sysroot)
        .env(
            format!("CMAKE_TOOLCHAIN_FILE_{underscored}"),
            &wrappers.cmake_toolchain,
        )
        .env("TARGET_CMAKE_TOOLCHAIN_FILE", &wrappers.cmake_toolchain)
        .env(
            format!("BINDGEN_EXTRA_CLANG_ARGS_{underscored}"),
            format!(
                "--target={} --sysroot={}",
                invocation.target.cargo_triple(),
                toolchain.manifest.sysroot.display()
            ),
        )
        .env(
            "PATH",
            prepend_path(&wrappers.directory, std::env::var_os("PATH").as_deref())?,
        )
        .env_remove("CARGO");
    Ok(())
}

fn ensure_rust_target(target: &str, offline: bool, trace: bool) -> anyhow::Result<()> {
    let list = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output();
    let installed = list
        .as_ref()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line == target)
        });
    if installed {
        return Ok(());
    }
    if offline {
        bail!("Rust target `{target}` is not installed and offline mode forbids downloading it");
    }
    if trace {
        eprintln!("[cargo-xc trace] rustup target add {target}");
    }
    let status = Command::new("rustup")
        .args(["target", "add", target])
        .status()?;
    if !status.success() {
        bail!("rustup failed to install target `{target}`");
    }
    Ok(())
}

fn should_verify(operation: &str) -> bool {
    matches!(operation, "build" | "rustc")
}

fn artifact_root(cargo: &OsStr, invocation: &CargoInvocation) -> anyhow::Result<PathBuf> {
    if let Some(path) = option_value(&invocation.args, "--target-dir") {
        let path = PathBuf::from(path);
        return Ok(if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        }
        .join(invocation.target.cargo_triple())
        .join(profile(&invocation.args)));
    }
    let mut metadata = Command::new(cargo);
    metadata.args(["metadata", "--no-deps", "--format-version", "1"]);
    if let Some(path) = option_value(&invocation.args, "--manifest-path") {
        metadata.arg("--manifest-path").arg(path);
    }
    metadata.env_remove("CARGO");
    let output = metadata.output()?;
    if !output.status.success() {
        bail!("failed to determine Cargo target directory after build");
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let target_dir = value["target_directory"]
        .as_str()
        .context("cargo metadata omitted target_directory")?;
    Ok(Path::new(target_dir)
        .join(invocation.target.cargo_triple())
        .join(profile(&invocation.args)))
}

fn profile(args: &[OsString]) -> String {
    option_value(args, "--profile")
        .and_then(OsStr::to_str)
        .map_or_else(
            || {
                if args.iter().any(|arg| arg == "--release") {
                    "release".into()
                } else {
                    "debug".into()
                }
            },
            ToOwned::to_owned,
        )
}

fn option_value<'a>(args: &'a [OsString], name: &str) -> Option<&'a OsStr> {
    for (index, argument) in args.iter().enumerate() {
        if argument == name {
            return args.get(index + 1).map(OsString::as_os_str);
        }
        if let Some(value) = argument
            .to_str()
            .and_then(|arg| arg.strip_prefix(&format!("{name}=")))
        {
            return Some(OsStr::new(value));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn defaults_to_build_and_forwards_arguments() {
        let parsed = CargoInvocation::parse(strings(&[
            "--release",
            "--package",
            "demo",
            "--target",
            "x86_64-unknown-linux-gnu.2.36",
        ]))
        .unwrap();
        assert_eq!(parsed.operation, "build");
        assert_eq!(
            parsed.args,
            strings(&[
                "--release",
                "--package",
                "demo",
                "--target",
                "x86_64-unknown-linux-gnu"
            ])
        );
    }

    #[test]
    fn recognizes_operation_and_equals_target() {
        let parsed = CargoInvocation::parse(strings(&[
            "check",
            "--workspace",
            "--target=x86_64-unknown-linux-gnu.2.36",
        ]))
        .unwrap();
        assert_eq!(parsed.operation, "check");
        assert_eq!(
            parsed.args,
            strings(&["--workspace", "--target=x86_64-unknown-linux-gnu"])
        );
    }

    #[test]
    fn rejects_unprovisioned_baseline() {
        let result =
            CargoInvocation::parse(strings(&["--target", "x86_64-unknown-linux-gnu.2.17"]));
        assert!(result.unwrap_err().to_string().contains("not provisioned"));
    }
}
