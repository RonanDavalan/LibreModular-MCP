#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_code))]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod audit;
pub mod cli;
pub mod config;
pub mod launch;
pub mod modules;
pub mod output;
pub mod protocol;
pub mod registry;
pub mod security;
pub mod server;
#[cfg(windows)]
mod windows_api;
