#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    libremodular_mcp::cli::main_entry()
}
