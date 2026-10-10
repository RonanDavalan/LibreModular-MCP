//! Point d'entrée de la commande `libremodular-mcp`.
//!
//! Pourquoi ce module existe
//! Le binaire reste une enveloppe d'une ligne autour de la bibliothèque, pour que chaque
//! comportement de la commande se teste sans démarrer de processus.

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    libremodular_mcp::cli::main_entry()
}
