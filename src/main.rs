//! Entry point of the `libremodular-mcp` command.
//!
//! Why this module exists
//! The binary stays a one-line shell around the library, so that every behaviour of
//! the command can be tested without starting a process.

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    libremodular_mcp::cli::main_entry()
}
