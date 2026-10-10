use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::{Context, bail};
use goblin::elf::header::EM_X86_64;
use regex::Regex;
use semver::Version;
use walkdir::WalkDir;

use crate::target::Target;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Verification {
    pub path: PathBuf,
    pub architecture: String,
    pub highest_glibc: Option<Version>,
    pub interpreter: Option<String>,
}

pub fn verify_file(path: &Path, target: &Target, readelf: &Path) -> anyhow::Result<Verification> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let goblin::Object::Elf(elf) = goblin::Object::parse(&bytes)? else {
        bail!("{} is not an ELF binary", path.display());
    };
    if elf.header.e_machine != EM_X86_64 || !elf.is_64 {
        bail!(
            "{} has the wrong architecture (ELF machine {}, 64-bit {}); expected ELF64 x86-64",
            path.display(),
            elf.header.e_machine,
            elf.is_64
        );
    }

    let output = Command::new(readelf)
        .args(["-W", "--version-info", "--program-headers"])
        .arg(path)
        .output()
        .with_context(|| format!("failed to run {}", readelf.display()))?;
    if !output.status.success() {
        bail!(
            "readelf failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let highest_glibc = highest_glibc_version(&text)?;
    if let Some(version) = &highest_glibc
        && version > &target.glibc
    {
        bail!(
            "{} imports GLIBC_{}.{} but requested baseline is {}; the selected sysroot or a native dependency is too new",
            path.display(),
            version.major,
            version.minor,
            target.glibc_short()
        );
    }
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
        && value != "/lib64/ld-linux-x86-64.so.2"
    {
        bail!(
            "{} requests unexpected dynamic loader `{value}`",
            path.display()
        );
    }
    Ok(Verification {
        path: path.to_path_buf(),
        architecture: "ELF64 x86-64".into(),
        highest_glibc,
        interpreter,
    })
}

pub fn verify_recent_artifacts(
    root: &Path,
    since: SystemTime,
    target: &Target,
    readelf: &Path,
) -> anyhow::Result<Vec<Verification>> {
    let mut verified = Vec::new();
    if !root.exists() {
        return Ok(verified);
    }
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let metadata = entry.metadata()?;
        if metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH) < since {
            continue;
        }
        let prefix = fs::read(entry.path()).unwrap_or_default();
        if !prefix.starts_with(b"\x7fELF") {
            continue;
        }
        let object = goblin::Object::parse(&prefix)?;
        if let goblin::Object::Elf(elf) = object
            && elf.header.e_machine == EM_X86_64
            && (elf.entry != 0 || elf.is_lib)
        {
            verified.push(verify_file(entry.path(), target, readelf)?);
        }
    }
    verified.sort_by(|a, b| a.path.cmp(&b.path));
    verified.dedup_by(|a, b| a.path == b.path);
    Ok(verified)
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
