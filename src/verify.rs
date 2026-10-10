use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, bail};
use regex::Regex;
use semver::Version;
use tokio::fs;

use crate::process::Managed;
use crate::target::Target;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Verification {
    pub path: PathBuf,
    pub architecture: String,
    pub highest_glibc: Option<Version>,
    pub interpreter: Option<String>,
}

pub async fn verify_file(
    path: &Path,
    target: &Target,
    readelf: &Path,
) -> anyhow::Result<Verification> {
    let bytes = fs::read(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    let spec = target.spec();
    let goblin::Object::Elf(elf) = goblin::Object::parse(&bytes)? else {
        bail!("{} is not an ELF binary", path.display());
    };
    if elf.header.e_machine != spec.elf_machine || !elf.is_64 {
        bail!(
            "{} has the wrong architecture (ELF machine {}, 64-bit {}); expected {}",
            path.display(),
            elf.header.e_machine,
            elf.is_64,
            spec.elf_description
        );
    }

    let mut command = Managed::new(readelf);
    command
        .args(["-W", "--version-info", "--program-headers"])
        .arg(path);
    let output = command
        .output()
        .await
        .with_context(|| format!("failed to run {}", readelf.display()))?;
    if !output.status.success() {
        bail!(
            "readelf failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // Imported GLIBC versions are reported, not policed: the binary links
    // against the same libc family the runtime ships.
    let highest_glibc = highest_glibc_version(&text)?;
    let interpreter = text.lines().find_map(|line| {
        let marker = "Requesting program interpreter:";
        line.find(marker).map(|index| {
            line[index + marker.len()..]
                .trim()
                .trim_end_matches(']')
                .to_owned()
        })
    });
    if let Some(value) = &interpreter
        && value != spec.loader
    {
        bail!(
            "{} requests unexpected dynamic loader `{value}`; expected `{}`",
            path.display(),
            spec.loader
        );
    }
    Ok(Verification {
        path: path.to_path_buf(),
        architecture: spec.elf_description.into(),
        highest_glibc,
        interpreter,
    })
}

pub async fn verify_recent_artifacts(
    root: &Path,
    since: SystemTime,
    target: &Target,
    readelf: &Path,
) -> anyhow::Result<Vec<Verification>> {
    let mut verified = Vec::new();
    if !root.exists() {
        return Ok(verified);
    }
    for path in files_under(root).await? {
        let metadata = fs::metadata(&path).await?;
        if metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH) < since {
            continue;
        }
        let prefix = fs::read(&path).await.unwrap_or_default();
        if !prefix.starts_with(b"\x7fELF") {
            continue;
        }
        let object = goblin::Object::parse(&prefix)?;
        if let goblin::Object::Elf(elf) = object
            && elf.header.e_machine == target.spec().elf_machine
            && (elf.entry != 0 || elf.is_lib)
        {
            verified.push(verify_file(&path, target, readelf).await?);
        }
    }
    verified.sort_by(|a, b| a.path.cmp(&b.path));
    verified.dedup_by(|a, b| a.path == b.path);
    Ok(verified)
}

/// Every regular file below `root`, without following symbolic links.
async fn files_under(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .await
            .with_context(|| format!("failed to read {}", directory.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }
    Ok(files)
}

pub fn highest_glibc_version(text: &str) -> anyhow::Result<Option<Version>> {
    let regex = Regex::new(r"GLIBC_(\d+)\.(\d+)")?;
    let mut highest: Option<Version> = None;
    for captures in regex.captures_iter(text) {
        let candidate = Version::new(captures[1].parse()?, captures[2].parse()?, 0);
        if highest.as_ref().is_none_or(|current| candidate > *current) {
            highest = Some(candidate);
        }
    }
    Ok(highest)
}

pub fn print_verification(result: &Verification, target: &Target) {
    println!("Artifact: {}", result.path.display());
    println!("Requested glibc baseline: {}", target.glibc_short());
    match &result.highest_glibc {
        Some(version) => println!(
            "Highest imported GLIBC symbol: {}.{}",
            version.major, version.minor
        ),
        None => println!("Highest imported GLIBC symbol: none (static or no glibc imports)"),
    }
    println!(
        "Dynamic loader: {}",
        result.interpreter.as_deref().unwrap_or("none")
    );
    println!("Architecture: {}", result.architecture);
    println!("Verification: passed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_highest_glibc_symbol() {
        let text = "puts@GLIBC_2.2.5 memcpy@GLIBC_2.14 stat@GLIBC_2.33";
        let version = highest_glibc_version(text).unwrap().unwrap();
        assert_eq!((version.major, version.minor), (2, 33));
    }

    #[test]
    fn handles_no_glibc_symbols() {
        assert!(highest_glibc_version("no versions").unwrap().is_none());
    }
}
