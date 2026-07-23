#![doc = include_str!("../README.md")]
// `Error` carries a large ociman variant (`PushError`); boxing it to shrink every
// `Result<_, Error>` is not worth the ergonomic cost for a CLI.
#![allow(clippy::result_large_err)]

pub mod cli;
pub mod debian;
pub mod patches;
pub mod target;
pub mod upstream;

mod error;

pub use error::Error;

#[must_use]
#[tokio::main(flavor = "current_thread")]
pub async fn main() -> std::process::ExitCode {
    use clap::Parser;

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    match cli::App::parse().run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                eprintln!("  caused by: {cause}");
                source = cause.source();
            }
            std::process::ExitCode::FAILURE
        }
    }
}
