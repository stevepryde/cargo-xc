//! The one place cargo-xc spawns child processes.
//!
//! Every child (`docker`, `cargo`, `rustup`, the cross tools, version probes)
//! goes through [`Managed`], so all of them share one lifecycle: `kill_on_drop`
//! at the spawn site, completion raced against Ctrl-C, and on cancellation a
//! graceful `SIGTERM`, a bounded grace period, then `SIGKILL`. Long-running
//! children such as `docker run`/`docker build` hold external resources (a
//! container, a `BuildKit` session), so they must never outlive an interrupted
//! run.

use std::ffi::OsStr;
use std::fmt;
use std::process::{ExitStatus, Output, Stdio};
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::Context;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::watch;

/// How long a child gets between `SIGTERM` and `SIGKILL` once a run is cancelled.
const TERMINATE_GRACE: Duration = Duration::from_secs(10);

/// Process-wide cancellation flag; flipped once by [`watch_ctrl_c`].
static CANCEL: LazyLock<watch::Sender<bool>> = LazyLock::new(|| watch::channel(false).0);

/// The run was interrupted (Ctrl-C) while a managed child was running or
/// about to be started. `main` maps this to exit status 130.
#[derive(Clone, Copy, Debug)]
pub struct Interrupted;

impl fmt::Display for Interrupted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("interrupted")
    }
}

impl std::error::Error for Interrupted {}

/// Turn the process's Ctrl-C (and, on Unix, `SIGTERM`) into cancellation of
/// every managed child.
///
/// The first signal flips the cancellation flag: running children are
/// terminated gracefully and no further children are started. A second
/// signal exits immediately. `SIGTERM` matters when this binary is PID 1
/// inside the builder container: the docker CLI forwards the host's
/// `SIGTERM` there, and without a handler PID 1 would drop it and the inner
/// `cargo` would keep running. Call once, from `main`, before dispatching.
pub fn watch_ctrl_c() {
    tokio::spawn(async {
        if interrupt_signal().await.is_err() {
            return;
        }
        cancel();
        if interrupt_signal().await.is_ok() {
            std::process::exit(130);
        }
    });
}

#[cfg(unix)]
async fn interrupt_signal() -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        _ = term.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn interrupt_signal() -> std::io::Result<()> {
    tokio::signal::ctrl_c().await
}

/// Request cancellation of every current and future managed child.
pub fn cancel() {
    // `send` is dropped when nobody is subscribed (a Ctrl-C landing between
    // two children); `send_replace` always stores the value.
    CANCEL.send_replace(true);
}

/// Whether cancellation has been requested for this process.
#[must_use]
pub fn cancelled() -> bool {
    *CANCEL.borrow()
}

/// A child process builder that always spawns through the managed lifecycle.
pub struct Managed {
    command: Command,
}

impl Managed {
    #[must_use]
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        let mut command = Command::new(program);
        command.kill_on_drop(true);
        Self { command }
    }

    pub fn arg(&mut self, argument: impl AsRef<OsStr>) -> &mut Self {
        self.command.arg(argument);
        self
    }

    pub fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command.args(arguments);
        self
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.command.env(key, value);
        self
    }

    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.command.env_remove(key);
        self
    }

    /// Run to completion with the parent's stdio, so `cargo`/`docker` output
    /// streams to the terminal. Fails with [`Interrupted`] on Ctrl-C after the
    /// child has been terminated.
    pub async fn status(mut self) -> anyhow::Result<ExitStatus> {
        let mut child = self.spawn()?;
        wait(&mut child).await
    }

    /// Run to completion capturing stdout and stderr (stdin is closed).
    /// Fails with [`Interrupted`] on Ctrl-C after the child has been terminated.
    pub async fn output(mut self) -> anyhow::Result<Output> {
        self.command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = self.spawn()?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (status, stdout, stderr) = tokio::join!(wait(&mut child), drain(stdout), drain(stderr));
        Ok(Output {
            status: status?,
            stdout: stdout?,
            stderr: stderr?,
        })
    }

    fn spawn(&mut self) -> anyhow::Result<Child> {
        if cancelled() {
            return Err(Interrupted.into());
        }
        self.command.spawn().map_err(Into::into)
    }
}

impl fmt::Debug for Managed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.command.as_std().fmt(formatter)
    }
}

async fn wait(child: &mut Child) -> anyhow::Result<ExitStatus> {
    let mut cancel = CANCEL.subscribe();
    tokio::select! {
        biased;
        _ = cancel.wait_for(|cancelled| *cancelled) => {
            terminate(child).await;
            Err(Interrupted.into())
        }
        status = child.wait() => status.context("failed to wait for child process"),
    }
}

/// `SIGTERM`, then `SIGKILL` if the child is still alive after the grace period.
async fn terminate(child: &mut Child) {
    if let Some(pid) = child.id() {
        // `unsafe_code = "forbid"` rules out libc::kill; kill(1) is on every
        // supported host and delivers the graceful signal for us.
        let mut kill = Command::new("kill");
        kill.arg("-TERM")
            .arg(pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let sent = kill.status().await.is_ok_and(|status| status.success());
        if sent
            && tokio::time::timeout(TERMINATE_GRACE, child.wait())
                .await
                .is_ok()
        {
            return;
        }
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

async fn drain<R: AsyncRead + Unpin>(pipe: Option<R>) -> anyhow::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_end(&mut buffer)
            .await
            .context("failed to read child process output")?;
    }
    Ok(buffer)
}
