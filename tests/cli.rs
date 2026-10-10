use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::process::{Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[test]
fn cargo_subcommand_help_uses_xc_identity() {
    Command::cargo_bin("cargo-xc")
        .unwrap()
        .args(["xc", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cargo-xc"))
        .stdout(predicate::str::contains("cargo xc docker"))
        .stdout(predicate::str::contains("cargo crossbuild").not());
}

#[test]
fn unsupported_baseline_is_actionable() {
    Command::cargo_bin("cargo-xc")
        .unwrap()
        .args(["xc", "build", "--target", "x86_64-unknown-linux-gnu.2.17"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not provisioned"))
        .stderr(predicate::str::contains("never substitutes"));
}

#[test]
fn targets_only_advertises_verified_baselines() {
    Command::cargo_bin("cargo-xc")
        .unwrap()
        .args(["xc", "targets"])
        .assert()
        .success()
        .stdout(predicate::str::contains("x86_64-unknown-linux-gnu.2.36"))
        .stdout(predicate::str::contains("aarch64-unknown-linux-gnu.2.34"))
        .stdout(predicate::str::contains(".2.17").not());
}

#[test]
fn lambda_rejects_the_amd64_target() {
    Command::cargo_bin("cargo-xc")
        .unwrap()
        .args(["xc", "lambda", "--target", "x86_64-unknown-linux-gnu.2.36"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("provided.al2023"))
        .stderr(predicate::str::contains("aarch64-unknown-linux-gnu"));
}

#[test]
fn ctrl_c_terminates_the_docker_child() {
    let root = std::env::temp_dir().join(format!(
        "cargo-xc-cli-docker-test-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let pid_file = root.join("docker.pid");
    let fake_bin =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-docker");
    let path = std::env::join_paths(
        std::iter::once(fake_bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();

    let mut xc = StdCommand::new(assert_cmd::cargo::cargo_bin("cargo-xc"))
        .args(["xc", "docker", "build", "--offline"])
        .env("PATH", path)
        .env("CARGO_XC_CACHE_DIR", root.join("cache"))
        .env("CARGO_XC_TEST_DOCKER_PIDFILE", &pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let docker_pid = wait_for(Duration::from_secs(20), || {
        fs::read_to_string(&pid_file)
            .ok()
            .and_then(|pid| pid.trim().parse::<u32>().ok())
    })
    .expect("fake docker never started");
    assert!(alive(docker_pid));

    signal(xc.id(), "-INT");
    let status = wait_for(Duration::from_secs(20), || xc.try_wait().unwrap())
        .expect("cargo-xc did not exit after Ctrl-C");
    assert_eq!(status.code(), Some(130));
    assert!(
        wait_for(Duration::from_secs(5), || (!alive(docker_pid))
            .then_some(()))
        .is_some(),
        "docker child outlived cargo-xc"
    );

    fs::remove_dir_all(root).unwrap();
}

fn wait_for<T>(limit: Duration, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(value) = probe() {
            return Some(value);
        }
        if Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn signal(pid: u32, signal: &str) {
    assert!(
        StdCommand::new("kill")
            .args([signal, &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
}

fn alive(pid: u32) -> bool {
    StdCommand::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}
