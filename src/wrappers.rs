use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::cache::Cache;
use crate::target::Target;
use crate::toolchain::Toolchain;

#[derive(Clone, Debug)]
pub struct Wrappers {
    pub directory: PathBuf,
    pub programs: BTreeMap<String, PathBuf>,
    pub cmake_toolchain: PathBuf,
}

impl Wrappers {
    pub fn prepare(cache: &Cache, target: &Target, toolchain: &Toolchain) -> anyhow::Result<Self> {
        let _lock = cache.lock()?;
        let directory = cache.wrappers(&toolchain.manifest.identity);
        fs::create_dir_all(&directory)?;
        let mut programs = BTreeMap::new();
        for (role, conventional) in [
            ("cc", "x86_64-linux-gnu-cc"),
            ("cxx", "x86_64-linux-gnu-c++"),
            ("cc", "x86_64-linux-gnu-gcc"),
            ("cxx", "x86_64-linux-gnu-g++"),
            ("ar", "x86_64-linux-gnu-ar"),
            ("ranlib", "x86_64-linux-gnu-ranlib"),
            ("strip", "x86_64-linux-gnu-strip"),
            ("objcopy", "x86_64-linux-gnu-objcopy"),
            ("nm", "x86_64-linux-gnu-nm"),
            ("readelf", "x86_64-linux-gnu-readelf"),
        ] {
            let path = directory.join(conventional);
            write_exec_wrapper(&path, toolchain.tool(role)?, &[])?;
            programs.entry(role.to_owned()).or_insert(path);
        }
        let pkg_config = directory.join("x86_64-linux-gnu-pkg-config");
        write_pkg_config_wrapper(&pkg_config, &toolchain.manifest.sysroot)?;
        programs.insert("pkg_config".into(), pkg_config);
        let cmake_toolchain = directory.join("cargo-xc-toolchain.cmake");
        write_cmake_toolchain(&cmake_toolchain, &programs, &toolchain.manifest.sysroot)?;
        fs::write(
            directory.join("identity"),
            format!("{}\n", target.qualified()),
        )?;
        Ok(Self {
            directory,
            programs,
            cmake_toolchain,
        })
    }

    pub fn program(&self, role: &str) -> anyhow::Result<&Path> {
        self.programs
            .get(role)
            .map(PathBuf::as_path)
            .with_context(|| format!("wrapper `{role}` is unavailable"))
    }
}

fn write_exec_wrapper(path: &Path, program: &Path, fixed_args: &[&str]) -> anyhow::Result<()> {
    let mut content = format!(
        "#!/bin/sh\nexec {}",
        quote(program.as_os_str().to_string_lossy().as_ref())
    );
    for argument in fixed_args {
        content.push(' ');
        content.push_str(&quote(argument));
    }
    content.push_str(" \"$@\"\n");
    write_executable(path, content.as_bytes())
}

fn write_pkg_config_wrapper(path: &Path, sysroot: &Path) -> anyhow::Result<()> {
    let pkg_config = find_in_path("pkg-config").context(
        "pkg-config is required for target dependency discovery; install it in the builder image",
    )?;
    let root = quote(sysroot.as_os_str().to_string_lossy().as_ref());
    let libdir = format!(
        "{}/usr/lib/x86_64-linux-gnu/pkgconfig:{}/usr/lib/pkgconfig:{}/usr/share/pkgconfig",
        sysroot.display(),
        sysroot.display(),
        sysroot.display()
    );
    let content = format!(
        "#!/bin/sh\nexport PKG_CONFIG_SYSROOT_DIR={root}\nexport PKG_CONFIG_LIBDIR={}\nunset PKG_CONFIG_PATH\nexec {} \"$@\"\n",
        quote(&libdir),
        quote(pkg_config.as_os_str().to_string_lossy().as_ref())
    );
    write_executable(path, content.as_bytes())
}

fn write_cmake_toolchain(
    path: &Path,
    programs: &BTreeMap<String, PathBuf>,
    sysroot: &Path,
) -> anyhow::Result<()> {
    let get = |role: &str| -> anyhow::Result<String> {
        Ok(programs
            .get(role)
            .with_context(|| format!("missing {role} wrapper"))?
            .display()
            .to_string())
    };
    let content = format!(
        "set(CMAKE_SYSTEM_NAME Linux)\nset(CMAKE_SYSTEM_PROCESSOR x86_64)\nset(CMAKE_C_COMPILER \"{}\")\nset(CMAKE_CXX_COMPILER \"{}\")\nset(CMAKE_AR \"{}\")\nset(CMAKE_RANLIB \"{}\")\nset(CMAKE_STRIP \"{}\")\nset(CMAKE_SYSROOT \"{}\")\nset(CMAKE_FIND_ROOT_PATH \"{}\")\nset(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)\nset(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)\nset(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)\nset(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)\n",
        get("cc")?,
        get("cxx")?,
        get("ar")?,
        get("ranlib")?,
        get("strip")?,
        sysroot.display(),
        sysroot.display()
    );
    write_if_changed(path, content.as_bytes())
}

fn write_executable(path: &Path, content: &[u8]) -> anyhow::Result<()> {
    if fs::read(path).ok().as_deref() == Some(content) {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o755)
        .open(path)?;
    file.write_all(content)?;
    Ok(())
}

fn write_if_changed(path: &Path, content: &[u8]) -> anyhow::Result<()> {
    if fs::read(path).ok().as_deref() != Some(content) {
        fs::write(path, content)?;
    }
    Ok(())
}

fn quote(value: &str) -> String {
    if value.contains('\n') {
        bail_quote();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn bail_quote() -> ! {
    panic!("wrapper paths must not contain newlines")
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

pub fn prepend_path(
    directory: &Path,
    old_path: Option<&std::ffi::OsStr>,
) -> anyhow::Result<std::ffi::OsString> {
    let paths = std::iter::once(directory.to_path_buf())
        .chain(old_path.map(std::env::split_paths).into_iter().flatten());
    std::env::join_paths(paths).context("failed to construct PATH")
}
