//! `agit` command line interface.
//!
//! Every command supports `--json`. On success the JSON envelope is
//! `{"ok":true,"result":...}`; on failure `{"ok":false,"error":{"code",
//! "message","retryable"}}`. Exit codes: 0 ok, 1 error, 3 conflict
//! (merge/lease/dirty worktree), 75 retryable (lock timeout, ref conflict).

mod cli;
mod output;
mod remote;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args = <cli::Cli as clap::Parser>::parse();
    let json = args.json;
    match cli::run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let code = output::report_error(&e, json);
            ExitCode::from(code)
        }
    }
}
