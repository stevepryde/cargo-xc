use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::Context;
use fs2::FileExt;

#[derive(Clone, Debug)]
pub struct Cache {
    root: PathBuf,
}

pub struct CacheLock {
    file: File,
}

impl Cache {
    pub fn discover() -> anyhow::Result<Self> {
        let root = std::env::var_os("CARGO_XC_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::cache_dir().map(|path| path.join("cargo-xc")))
            .context("cannot determine a cache directory; set CARGO_XC_CACHE_DIR")?;
        Ok(Self { root })
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn toolchains(&self) -> PathBuf {
        self.root.join("toolchains")
    }

    #[must_use]
    pub fn toolchain(&self, identity: &str) -> PathBuf {
        self.toolchains().join(identity)
    }

    #[must_use]
    pub fn wrappers(&self, identity: &str) -> PathBuf {
        self.root.join("wrappers").join(identity)
    }

    pub fn ensure(&self) -> anyhow::Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create cache {}", self.root.display()))
    }

    pub fn lock(&self) -> anyhow::Result<CacheLock> {
        self.ensure()?;
        let path = self.root.join(".lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        file.lock_exclusive()
            .with_context(|| format!("failed to lock {}", path.display()))?;
        Ok(CacheLock { file })
    }

    pub fn clean(&self) -> anyhow::Result<()> {
        let _lock = self.lock()?;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_name() == ".lock" {
                continue;
            }
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(path)?;
            } else {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
