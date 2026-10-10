//! Point d'entrée de la commande `libremodular-launch`.
//!
//! Pourquoi ce module existe
//! Le lanceur est un binaire à part pour que sa liste d'arguments soit fixée par le code
//! et ne puisse jamais être construite à partir d'un texte écrit par le modèle.

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    libremodular_mcp::cli::launch_entry()
}
