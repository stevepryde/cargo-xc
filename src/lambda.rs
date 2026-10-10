use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};
use tokio::fs;

use crate::cache::Cache;
use crate::cargo_proxy::{CargoInvocation, artifact_root, execute, target_directory};
use crate::target::{AARCH64_TARGET, Target};
use crate::toolchain::Toolchain;
use crate::verify::verify_file;
use crate::wrappers::Wrappers;

/// `cargo xc lambda`: build for the AWS Lambda `provided.al2023` ARM64
/// runtime, verify the artifact, and package it as a `bootstrap` zip.
pub async fn run(args: &[OsString], cache: &Cache, trace: bool) -> anyhow::Result<u8> {
    let mut args: Vec<OsString> = args.to_vec();
    let zip_override = take_option(&mut args, "--zip")?.map(PathBuf::from);
    let requested_bin = peek_option(&args, "--bin");

    match crate::target::find_target_argument(&args)? {
        Some(target) if target.cargo_triple() != AARCH64_TARGET => bail!(
            "`cargo xc lambda` targets the ARM64 `provided.al2023` runtime (glibc 2.34); the {} toolchain's glibc {} baseline is not Lambda-compatible. Omit --target or pass {AARCH64_TARGET}[.2.34]",
            target.cargo_triple(),
            target.glibc_short()
        ),
        Some(_) => {}
        None => {
            args.push("--target".into());
            args.push(Target::lambda_default().qualified().into());
        }
    }
    let has_profile = args
        .iter()
        .any(|arg| arg == "--release" || arg == "--profile" || is_prefixed(arg, "--profile="));
    if !has_profile {
        args.insert(0, "--release".into());
    }

    let offline = args
        .iter()
        .any(|arg| arg == "--offline" || arg == "--frozen");
    let mut build_args = vec![OsString::from("build")];
    build_args.extend(args.iter().cloned());
    let invocation = CargoInvocation::parse(build_args)?;
    let code = execute(&invocation, cache, offline, trace).await?;
    if code != 0 {
        return Ok(code);
    }

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let root = artifact_root(&cargo, &invocation).await?;
    let binary = select_binary(&root, &invocation.target, requested_bin.as_deref()).await?;
    let toolchain = Toolchain::load(cache, &invocation.target).await?;
    let wrappers = Wrappers::prepare(cache, &invocation.target, &toolchain).await?;
    verify_file(&binary, &invocation.target, wrappers.program("readelf")?).await?;

    let name = binary
        .file_name()
        .context("binary has no file name")?
        .to_string_lossy()
        .into_owned();
    let zip_path = match zip_override {
        Some(path) if path.is_absolute() => path,
        Some(path) => std::env::current_dir()?.join(path),
        None => target_directory(&cargo, &invocation.args)
            .await?
            .join("lambda")
            .join(&name)
            .join("bootstrap.zip"),
    };
    let checksum = write_bootstrap_zip(&binary, &zip_path).await?;
    let size = fs::metadata(&zip_path).await?.len();
    println!("Lambda package: {}", zip_path.display());
    println!("Bootstrap binary: {}", binary.display());
    println!("Package SHA-256: {checksum}");
    println!("Package size: {size} bytes");
    println!("Deploy with runtime `provided.al2023` and architecture `arm64`");
    Ok(0)
}

/// The one executable this build produced, or the one named by `--bin`.
async fn select_binary(
    root: &Path,
    target: &Target,
    requested: Option<&std::ffi::OsStr>,
) -> anyhow::Result<PathBuf> {
    let mut candidates = Vec::new();
    let mut entries = fs::read_dir(root)
        .await
        .with_context(|| format!("failed to read build output {}", root.display()))?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !entry.file_type().await?.is_file()
            || path.extension().is_some_and(|extension| extension == "so")
        {
            continue;
        }
        let bytes = fs::read(&path).await.unwrap_or_default();
        if let Ok(goblin::Object::Elf(elf)) = goblin::Object::parse(&bytes)
            && elf.header.e_machine == target.spec().elf_machine
            && elf.entry != 0
            && !elf.is_lib
        {
            candidates.push(path);
        }
    }
    candidates.sort();
    if let Some(name) = requested {
        return candidates
            .into_iter()
            .find(|path| path.file_name() == Some(name))
            .with_context(|| {
                format!(
                    "binary `{}` was not produced under {}",
                    name.to_string_lossy(),
                    root.display()
                )
            });
    }
    match candidates.as_slice() {
        [] => bail!(
            "no executable was produced under {}; is this a binary crate?",
            root.display()
        ),
        [only] => Ok(only.clone()),
        many => bail!(
            "multiple executables were produced ({}); select one with --bin NAME",
            many.iter()
                .filter_map(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Zip `binary` as a single executable `bootstrap` entry, deterministically.
async fn write_bootstrap_zip(binary: &Path, zip_path: &Path) -> anyhow::Result<String> {
    if let Some(parent) = zip_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let bytes = fs::read(binary)
        .await
        .with_context(|| format!("failed to read {}", binary.display()))?;
    let destination = zip_path.to_path_buf();
    // async-guard: allow spawn-blocking: the zip writer has no async API
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let file = std::fs::File::create(&destination)
            .with_context(|| format!("failed to create {}", destination.display()))?;
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o755)
            .last_modified_time(zip::DateTime::default());
        writer.start_file("bootstrap", options)?;
        writer.write_all(&bytes)?;
        writer.finish()?;
        Ok(())
    })
    .await
    .context("zip task panicked")??;
    let mut hasher = Sha256::new();
    hasher.update(fs::read(zip_path).await?);
    Ok(hex::encode(hasher.finalize()))
}

fn take_option(args: &mut Vec<OsString>, name: &str) -> anyhow::Result<Option<OsString>> {
    let mut index = 0;
    while index < args.len() {
        if args[index] == name {
            let value = args
                .get(index + 1)
                .with_context(|| format!("{name} requires a value"))?
                .clone();
            args.drain(index..=index + 1);
            return Ok(Some(value));
        }
        if let Some(value) = args[index]
            .to_str()
            .and_then(|arg| arg.strip_prefix(&format!("{name}=")))
        {
            let value = OsString::from(value);
            args.remove(index);
            return Ok(Some(value));
        }
        index += 1;
    }
    Ok(None)
}

fn peek_option(args: &[OsString], name: &str) -> Option<OsString> {
    for (index, argument) in args.iter().enumerate() {
        if argument == name {
            return args.get(index + 1).cloned();
        }
        if let Some(value) = argument
            .to_str()
            .and_then(|arg| arg.strip_prefix(&format!("{name}=")))
        {
            return Some(value.into());
        }
    }
    None
}

fn is_prefixed(argument: &OsString, prefix: &str) -> bool {
    argument.to_str().is_some_and(|arg| arg.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_option_removes_both_forms() {
        let mut args: Vec<OsString> = vec!["--zip".into(), "out.zip".into(), "--release".into()];
        let value = take_option(&mut args, "--zip").unwrap().unwrap();
        assert_eq!(value, "out.zip");
        assert_eq!(args, vec![OsString::from("--release")]);

        let mut args: Vec<OsString> = vec!["--zip=out.zip".into()];
        let value = take_option(&mut args, "--zip").unwrap().unwrap();
        assert_eq!(value, "out.zip");
        assert!(args.is_empty());
    }

    #[tokio::test]
    async fn bootstrap_zip_holds_one_executable_entry() {
        let root =
            std::env::temp_dir().join(format!("cargo-xc-lambda-zip-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("my-function");
        std::fs::write(&binary, b"\x7fELF-not-really").unwrap();
        let zip_path = root.join("out/bootstrap.zip");
        let checksum = write_bootstrap_zip(&binary, &zip_path).await.unwrap();
        assert_eq!(checksum.len(), 64);

        let file = std::fs::File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert_eq!(archive.len(), 1);
        let entry = archive.by_index(0).unwrap();
        assert_eq!(entry.name(), "bootstrap");
        assert_eq!(entry.unix_mode().unwrap() & 0o777, 0o755);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
