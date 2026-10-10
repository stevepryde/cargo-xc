use std::process::ExitCode;

use cargo_xc::process::{self, Interrupted};

const INTERRUPTED: u8 = 130;

#[tokio::main]
async fn main() -> ExitCode {
    process::watch_ctrl_c();
    let result = cargo_xc::cli::run(std::env::args_os()).await;
    if process::cancelled() || result.as_ref().is_err_and(anyhow::Error::is::<Interrupted>) {
        eprintln!("cargo-xc: interrupted");
        return ExitCode::from(INTERRUPTED);
    }
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
