use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;

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
    pub async fn prepare(
        cache: &Cache,
        target: &Target,
        toolchain: &Toolchain,
    ) -> anyhow::Result<Self> {
        let _lock = cache.lock().await?;
        let spec = target.spec();
        let directory = cache.wrappers(&toolchain.manifest.identity);
        fs::create_dir_all(&directory).await?;
        let mut programs = BTreeMap::new();
        for (role, suffix) in [
            ("cc", "cc"),
            ("cxx", "c++"),
            ("cc", "gcc"),
            ("cxx", "g++"),
            ("ar", "ar"),
            ("ranlib", "ranlib"),
            ("strip", "strip"),
            ("objcopy", "objcopy"),
            ("nm", "nm"),
            ("readelf", "readelf"),
        ] {
            let path = directory.join(format!("{}-{suffix}", spec.gnu_prefix));
            write_exec_wrapper(&path, toolchain.tool(role)?, &[]).await?;
            programs.entry(role.to_owned()).or_insert(path);
        }
        let pkg_config = directory.join(format!("{}-pkg-config", spec.gnu_prefix));
        write_pkg_config_wrapper(
            &pkg_config,
            &toolchain.manifest.sysroot,
            spec.pkg_config_dirs,
        )
        .await?;
        programs.insert("pkg_config".into(), pkg_config);
        let cmake_toolchain = directory.join("cargo-xc-toolchain.cmake");
        write_cmake_toolchain(
            &cmake_toolchain,
            &programs,
            &toolchain.manifest.sysroot,
            spec.architecture,
        )
        .await?;
        fs::write(
            directory.join("identity"),
            format!("{}\n", target.qualified()),
        )
        .await?;
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

async fn write_exec_wrapper(
    path: &Path,
    program: &Path,
    fixed_args: &[&str],
) -> anyhow::Result<()> {
    let mut content = format!(
        "#!/bin/sh\nexec {}",
        quote(program.as_os_str().to_string_lossy().as_ref())
    );
    for argument in fixed_args {
        content.push(' ');
        content.push_str(&quote(argument));
    }
    content.push_str(" \"$@\"\n");
    write_executable(path, content.as_bytes()).await
}

async fn write_pkg_config_wrapper(
    path: &Path,
    sysroot: &Path,
    pkg_config_dirs: &[&str],
) -> anyhow::Result<()> {
    let pkg_config = find_in_path("pkg-config").context(
        "pkg-config is required for target dependency discovery; install it in the builder image",
    )?;
    let root = quote(sysroot.as_os_str().to_string_lossy().as_ref());
    let libdir = pkg_config_dirs
        .iter()
        .map(|dir| format!("{}/{dir}", sysroot.display()))
        .collect::<Vec<_>>()
        .join(":");
    let content = format!(
        "#!/bin/sh\nexport PKG_CONFIG_SYSROOT_DIR={root}\nexport PKG_CONFIG_LIBDIR={}\nunset PKG_CONFIG_PATH\nexec {} \"$@\"\n",
        quote(&libdir),
        quote(pkg_config.as_os_str().to_string_lossy().as_ref())
    );
    write_executable(path, content.as_bytes()).await
}

async fn write_cmake_toolchain(
    path: &Path,
    programs: &BTreeMap<String, PathBuf>,
    sysroot: &Path,
    processor: &str,
) -> anyhow::Result<()> {
    let get = |role: &str| -> anyhow::Result<String> {
        Ok(programs
            .get(role)
            .with_context(|| format!("missing {role} wrapper"))?
            .display()
            .to_string())
    };
    let content = format!(
        "set(CMAKE_SYSTEM_NAME Linux)\nset(CMAKE_SYSTEM_PROCESSOR {processor})\nset(CMAKE_C_COMPILER \"{}\")\nset(CMAKE_CXX_COMPILER \"{}\")\nset(CMAKE_AR \"{}\")\nset(CMAKE_RANLIB \"{}\")\nset(CMAKE_STRIP \"{}\")\nset(CMAKE_SYSROOT \"{}\")\nset(CMAKE_FIND_ROOT_PATH \"{}\")\nset(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)\nset(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)\nset(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)\nset(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)\n",
        get("cc")?,
        get("cxx")?,
        get("ar")?,
        get("ranlib")?,
        get("strip")?,
        sysroot.display(),
        sysroot.display()
    );
    write_if_changed(path, content.as_bytes()).await
}

async fn write_executable(path: &Path, content: &[u8]) -> anyhow::Result<()> {
    if fs::read(path).await.ok().as_deref() == Some(content) {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o755)
        .open(path)
        .await?;
    file.write_all(content).await?;
    file.flush().await?;
    Ok(())
}

async fn write_if_changed(path: &Path, content: &[u8]) -> anyhow::Result<()> {
    if fs::read(path).await.ok().as_deref() != Some(content) {
        fs::write(path, content).await?;
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
