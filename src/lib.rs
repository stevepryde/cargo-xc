pub mod cache;
pub mod cargo_proxy;
pub mod cli;
pub mod docker;
pub mod doctor;
pub mod lambda;
pub mod lockfile;
pub mod process;
pub mod target;
pub mod toolchain;
pub mod verify;
pub mod wrappers;

pub const NAME: &str = "cargo-xc";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
