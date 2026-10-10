use assert_cmd::Command;
use predicates::prelude::*;

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
fn targets_only_advertises_verified_baseline() {
    Command::cargo_bin("cargo-xc")
        .unwrap()
        .args(["xc", "targets"])
        .assert()
        .success()
        .stdout(predicate::str::contains("x86_64-unknown-linux-gnu.2.36"))
        .stdout(predicate::str::contains(".2.17").not());
}
