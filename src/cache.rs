use std::path::{Path, PathBuf};

use anyhow::Context;
use fs2::FileExt;
use tokio::fs::{self, File, OpenOptions};

#[derive(Clone, Debug)]
pub struct Cache {
    root: PathBuf,
}

/// Exclusive advisory lock on the cache; released when dropped.
pub struct CacheLock {
    file: Option<File>,
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

    pub async fn ensure(&self) -> anyhow::Result<()> {
        fs::create_dir_all(&self.root)
            .await
            .with_context(|| format!("failed to create cache {}", self.root.display()))
    }

    pub async fn lock(&self) -> anyhow::Result<CacheLock> {
        self.ensure().await?;
        let path = self.root.join(".lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .await?
            .into_std()
            .await;
        // async-guard: allow spawn-blocking: fs2 flock has no async API
        let file = tokio::task::spawn_blocking(move || file.lock_exclusive().map(|()| file))
            .await
            .context("cache lock task panicked")?
            .with_context(|| format!("failed to lock {}", path.display()))?;
        Ok(CacheLock {
            file: Some(File::from_std(file)),
        })
    }

    pub async fn clean(&self) -> anyhow::Result<()> {
        let _lock = self.lock().await?;
        let mut entries = fs::read_dir(&self.root).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_name() == ".lock" {
                continue;
            }
            let path = entry.path();
            if entry.file_type().await?.is_dir() {
                fs::remove_dir_all(path).await?;
            } else {
                fs::remove_file(path).await?;
            }
        }
        Ok(())
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        // Closing the descriptor releases the flock; unlock explicitly when the
        // file is idle so the release does not wait on a background close.
        if let Some(Ok(file)) = self.file.take().map(File::try_into_std) {
            let _ = FileExt::unlock(&file);
        }
    }
}
