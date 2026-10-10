use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, bail};

use crate::cache::Cache;
use crate::cargo_proxy::CargoInvocation;
use crate::target::{RUST_TARGET, SUPPORTED_GLIBC, Target};
use crate::toolchain::Toolchain;

pub fn run(args: impl IntoIterator<Item = OsString>) -> anyhow::Result<u8> {
    let mut args: Vec<OsString> = args.into_iter().collect();
    if !args.is_empty() {
        args.remove(0);
    }
    if args.first().is_some_and(|arg| arg == "xc") {
        args.remove(0);
    }
    let trace = take_flag(&mut args, "--trace");
    let verbose = args.iter().any(|arg| arg == "--verbose" || arg == "-v");
    let cache = Cache::discover()?;
    match args.first().and_then(|arg| arg.to_str()) {
        None | Some("--help" | "-h" | "help") => {
            print_help();
            Ok(0)
        }
        Some("--version" | "-V") => {
            println!("cargo-xc {}", crate::VERSION);
            Ok(0)
        }
        Some("targets") => {
            println!("Supported and verified toolchain baselines:");
            for baseline in SUPPORTED_GLIBC {
                println!("  {RUST_TARGET}.{baseline}");
            }
            Ok(0)
        }
        Some("docker") => crate::docker::run(&args[1..], trace),
        Some("doctor") => {
            let target = management_target(&args[1..])?;
            let _ = crate::doctor::run(&cache, &target, verbose)?;
            Ok(0)
        }
        Some("init") => {
            let path = crate::lockfile::init(&std::env::current_dir()?)?;
            println!("Created {}", path.display());
            Ok(0)
        }
        Some("lock") => lock_command(&cache, &args[1..]),
        Some("verify") => verify_command(&cache, &args[1..]),
        Some("toolchain") => toolchain_command(&cache, &args[1..]),
        Some("cache") => cache_command(&cache, &args[1..]),
        _ => {
            let offline = args
                .iter()
                .any(|arg| arg == "--offline" || arg == "--frozen");
            let invocation = CargoInvocation::parse(args)?;
            crate::cargo_proxy::execute(&invocation, &cache, offline, trace)
        }
    }
}

fn toolchain_command(cache: &Cache, args: &[OsString]) -> anyhow::Result<u8> {
    match args.first().and_then(|arg| arg.to_str()) {
        Some("list") => {
            let manifests = crate::toolchain::installed(cache)?;
            if manifests.is_empty() {
                println!("No cargo-xc toolchains installed");
            } else {
                for manifest in manifests {
                    println!("{}\t{}", manifest.identity, manifest.sysroot.display());
                }
            }
        }
        Some("install") => {
            let target = positional_target(&args[1..])?;
            let toolchain = Toolchain::install_system(cache, &target)?;
            println!("Installed {}", toolchain.manifest.identity);
            println!("Manifest: {}", toolchain.manifest_path.display());
        }
        Some("inspect") => {
            let target = positional_target(&args[1..])?;
            let toolchain = Toolchain::load(cache, &target)?;
            println!("{}", serde_json::to_string_pretty(&toolchain.manifest)?);
        }
        Some("remove") => {
            let target = positional_target(&args[1..])?;
            let _lock = cache.lock()?;
            let path = cache.toolchain(&Toolchain::identity(&target));
            if path.exists() {
                fs::remove_dir_all(&path)?;
                let wrappers = cache.wrappers(&Toolchain::identity(&target));
                if wrappers.exists() {
                    fs::remove_dir_all(wrappers)?;
                }
                println!("Removed {}", target.qualified());
            } else {
                println!("Toolchain {} was not installed", target.qualified());
            }
        }
        _ => bail!("usage: cargo xc toolchain <list|install|inspect|remove> [TARGET]"),
    }
    Ok(0)
}

fn cache_command(cache: &Cache, args: &[OsString]) -> anyhow::Result<u8> {
    match args.first().and_then(|arg| arg.to_str()) {
        Some("clean") => {
            cache.clean()?;
            println!("Cleaned {}", cache.root().display());
            Ok(0)
        }
        _ => bail!("usage: cargo xc cache clean"),
    }
}

fn lock_command(cache: &Cache, args: &[OsString]) -> anyhow::Result<u8> {
    let target = management_target(args)?;
    let update = args.iter().any(|arg| arg == "--update");
    let toolchain = Toolchain::load(cache, &target)
        .with_context(|| format!("install {} before locking", target.qualified()))?;
    let path = crate::lockfile::write(&std::env::current_dir()?, &target, &toolchain, update)?;
    println!("Wrote {}", path.display());
    Ok(0)
}

fn verify_command(cache: &Cache, args: &[OsString]) -> anyhow::Result<u8> {
    let target = management_target(args)?;
    let toolchain = Toolchain::load(cache, &target)?;
    let path = management_positionals(args)
        .into_iter()
        .next()
        .map(PathBuf::from);
    if let Some(path) = path {
        let result = crate::verify::verify_file(&path, &target, toolchain.tool("readelf")?)?;
        crate::verify::print_verification(&result, &target);
    } else {
        crate::lockfile::verify(&std::env::current_dir()?, &toolchain)?;
        println!(
            "{} matches installed toolchain {}",
            crate::lockfile::LOCK_FILE,
            target.qualified()
        );
    }
    Ok(0)
}

fn positional_target(args: &[OsString]) -> anyhow::Result<Target> {
    match args.first() {
        Some(value) => value.to_str().context("target must be UTF-8")?.parse(),
        None => Ok(Target::default()),
    }
}

fn management_target(args: &[OsString]) -> anyhow::Result<Target> {
    for (index, argument) in args.iter().enumerate() {
        if argument == "--target" {
            return args
                .get(index + 1)
                .context("--target requires a value")?
                .to_str()
                .context("target must be UTF-8")?
                .parse();
        }
        if let Some(value) = argument
            .to_str()
            .and_then(|arg| arg.strip_prefix("--target="))
        {
            return value.parse();
        }
    }
    Ok(Target::default())
}

fn take_flag(args: &mut Vec<OsString>, flag: &str) -> bool {
    let found = args.iter().any(|arg| arg == flag);
    args.retain(|arg| arg != flag);
    found
}

fn management_positionals(args: &[OsString]) -> Vec<&OsStr> {
    let mut values = Vec::new();
    let mut skip_next = false;
    for argument in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if argument == "--target" {
            skip_next = true;
            continue;
        }
        if argument
            .to_str()
            .is_some_and(|value| value.starts_with('-'))
        {
            continue;
        }
        values.push(argument.as_os_str());
    }
    values
}

fn print_help() {
    println!(
        "cargo-xc {}\n\nNative ARM64 Linux to AMD64 GNU/Linux Cargo cross-compilation.\n\nUSAGE:\n    cargo xc [OPERATION] [CARGO OPTIONS] --target {RUST_TARGET}[.GLIBC]\n    cargo xc docker [OPERATION] [CARGO OPTIONS]\n    cargo xc <COMMAND>\n\nOPERATIONS:\n    build (default), check, rustc, test, bench, run, clippy, doc\n\nCOMMANDS:\n    targets                         List genuinely provisioned baselines\n    doctor [--target TARGET]        Diagnose the active builder and toolchain\n    docker <ARGS>                   Build inside a native linux/arm64 container\n    toolchain list                  List cached toolchains\n    toolchain install [TARGET]      Register installed native cross tools\n    toolchain inspect [TARGET]      Print the immutable manifest\n    toolchain remove [TARGET]       Remove a cached toolchain\n    cache clean                     Remove all cargo-xc cache data\n    init                            Create cargo-xc.toml\n    lock [--update]                 Write cargo-xc.lock\n    verify [FILE]                   Verify a lock or AMD64 ELF artifact\n\nGLOBAL OPTIONS:\n    --trace                         Print managed subprocess invocations\n    --offline, --frozen             Forbid provisioning and forward to Cargo\n    -h, --help                      Print help\n    -V, --version                   Print version",
        crate::VERSION
    );
}
