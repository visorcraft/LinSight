// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![deny(rust_2018_idioms)]
#![deny(unsafe_op_in_unsafe_fn)]

mod alerts;
mod hardware;
mod history;
mod nickname_store;
mod plugin_host;
mod prom;
mod runtime;
mod scheduler;
mod transport;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about = "LinSight sensor daemon")]
struct Cli {
    /// Override the Unix socket path. Defaults to $XDG_RUNTIME_DIR/linsight.sock.
    #[arg(long)]
    socket: Option<std::path::PathBuf>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LINSIGHT_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let socket = match cli.socket {
        Some(s) => s,
        None => default_socket_path()?,
    };
    runtime::run(socket)
}

fn default_socket_path() -> anyhow::Result<std::path::PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .map(|dir| dir.join("linsight.sock"))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "$XDG_RUNTIME_DIR is not set; refusing to fall back to /tmp. \
                 Set XDG_RUNTIME_DIR or pass --socket explicitly."
            )
        })
}
