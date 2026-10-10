use std::process::ExitCode;

fn main() -> ExitCode {
    match cargo_xc::cli::run(std::env::args_os()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
