//! Entry point of the `libremodular-launch` command.
//!
//! Why this module exists
//! The launcher is a separate binary so that its argument list is fixed by code and
//! can never be built from text the model wrote.

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    libremodular_mcp::cli::launch_entry()
}
