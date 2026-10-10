use std::process::Command;

use crate::cache::Cache;
use crate::target::Target;
use crate::toolchain::Toolchain;
use crate::wrappers::Wrappers;

pub fn run(cache: &Cache, target: &Target, verbose: bool) -> anyhow::Result<bool> {
    println!("cargo-xc doctor");
    println!("Host OS: {}", std::env::consts::OS);
    println!("Host architecture: {}", std::env::consts::ARCH);
    let native_arm64 = cfg!(target_os = "linux") && cfg!(target_arch = "aarch64");
    println!("Native ARM64 Linux builder: {}", yes_no(native_arm64));
    println!("Rust: {}", command_version("rustc"));
    println!("Cargo: {}", command_version("cargo"));
    println!("Selected target: {}", target.cargo_triple());
    println!("Selected glibc baseline: {}", target.glibc_short());
    println!("Cache: {}", cache.root().display());
    match Toolchain::load(cache, target) {
        Ok(toolchain) => {
            let wrappers = Wrappers::prepare(cache, target, &toolchain)?;
            println!("Compiler: {}", toolchain.tool("cc")?.display());
            println!("Compiler version: {}", toolchain.manifest.compiler_version);
            println!(
                "Linker/binutils version: {}",
                toolchain.manifest.linker_version
            );
            println!("Sysroot: {}", toolchain.manifest.sysroot.display());
            println!("Sysroot identity: {}", toolchain.manifest.identity);
            println!(
                "pkg-config wrapper: {}",
                wrappers.program("pkg_config")?.display()
            );
            println!("Cargo linker: {}", wrappers.program("cc")?.display());
            if verbose {
                println!("Tool manifest: {}", toolchain.manifest_path.display());
                for (role, path) in &toolchain.manifest.tools {
                    println!("Tool {role}: {}", path.display());
                }
            }
            println!("Host-library contamination: no host search paths configured");
            println!(
                "Readiness: {}",
                if native_arm64 {
                    "ready"
                } else {
                    "toolchain cached, but builds must run in `cargo xc docker`"
                }
            );
            Ok(native_arm64)
        }
        Err(error) => {
            println!("Toolchain: unavailable ({error})");
            println!("Readiness: not ready");
            Ok(false)
        }
    }
}

fn command_version(program: &str) -> String {
    Command::new(program)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map_or_else(
            || "unavailable".into(),
            |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        )
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
