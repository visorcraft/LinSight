// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(version, about = "LinSight command-line client")]
struct Cli {
    /// Override the Unix socket path. Defaults to $XDG_RUNTIME_DIR/linsight.sock.
    #[arg(long, global = true)]
    socket: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Print every sensor the daemon advertises.
    List,
    /// Subscribe to a single sensor and print samples until Ctrl+C.
    Read {
        sensor: String,
        /// Stop after N samples.
        #[arg(long)]
        count: Option<u64>,
    },
    /// Subscribe to one or more sensors and stream live formatted values.
    Watch {
        /// One or more sensor IDs to subscribe to.
        sensors: Vec<String>,

        /// Subscription rate in Hz (default: native sensor rate).
        #[arg(long)]
        rate: Option<f64>,

        /// Output format: plain or json.
        #[arg(long, default_value = "plain")]
        format: String,

        /// Stop after N samples (total across all sensors).
        #[arg(long)]
        count: Option<u64>,
    },
    /// Manage alert rules.
    Alert {
        #[command(subcommand)]
        action: AlertCmd,
    },
    /// Query sensor history from the daemon.
    History {
        /// Sensor id to query (e.g. cpu.util)
        sensor: String,
        /// How far back to query (e.g. "5m", "1h"). Default "5m".
        #[arg(long, default_value = "5m")]
        last: String,
        /// Output format: plain, csv, or json. Default "plain".
        #[arg(long, default_value = "plain")]
        format: String,
        /// Maximum data points to return.
        #[arg(long)]
        max_points: Option<u32>,
    },
    /// Offline history-database maintenance (no daemon required).
    Db {
        #[command(subcommand)]
        action: DbCmd,
    },
    /// Manage runtime plugins (.so files in `~/.local/share/linsight/plugins/`).
    Plugin {
        #[command(subcommand)]
        action: PluginCmd,
    },
}

#[derive(Subcommand, Debug)]
enum AlertCmd {
    /// List all alert rules with their status.
    List,
    /// Add or update an alert rule.
    Add {
        /// Rule name
        name: String,
        /// Alert expression (e.g. "cpu.util > 90")
        expr: String,
        /// Debounce: time condition must hold before firing (e.g. "30s")
        #[arg(long)]
        for_duration: Option<String>,
        /// Cooldown: minimum time between re-notifications (e.g. "5m")
        #[arg(long)]
        cooldown: Option<String>,
        /// Notify target (can be specified multiple times: "desktop", "exec:...", "webhook:...")
        #[arg(long)]
        notify: Vec<String>,
    },
    /// Remove an alert rule by name.
    Remove {
        /// Rule name
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum DbCmd {
    /// Print row count, distinct sensor count, timestamp span, and file size.
    Stats {
        /// Override the database path (default: $XDG_DATA_HOME/linsight/history.db).
        #[arg(long)]
        db: Option<std::path::PathBuf>,
    },
    /// Delete rows older than a given duration.
    Prune {
        /// Delete rows older than this duration (e.g. "7d", "24h", "90m").
        #[arg(long)]
        older_than: String,
        /// Run VACUUM after deleting to reclaim disk space.
        #[arg(long)]
        vacuum: bool,
        /// Override the database path (default: $XDG_DATA_HOME/linsight/history.db).
        #[arg(long)]
        db: Option<std::path::PathBuf>,
    },
    /// Export historical samples to CSV or JSON.
    Export {
        /// Output format: csv or json.
        #[arg(long, default_value = "csv")]
        format: String,
        /// Sensor id to export (e.g. cpu.util). If omitted, exports all sensors.
        #[arg(long)]
        sensor: Option<String>,
        /// How far back to export (e.g. "5m", "1h", "7d"). Default "24h".
        #[arg(long, default_value = "24h")]
        since: String,
        /// Override the database path (default: $XDG_DATA_HOME/linsight/history.db).
        #[arg(long)]
        db: Option<std::path::PathBuf>,
        /// Output file path. Defaults to stdout.
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum PluginCmd {
    /// Scaffold a new plugin crate at `<name>/`. Produces a `cdylib` Cargo
    /// project that depends on `linsight-plugin-sdk` and exposes a
    /// `LinsightPlugin` implementation via `export_plugin!`.
    New { name: String },
    /// Install a built plugin (.so) into `$XDG_DATA_HOME/linsight/plugins/`.
    Install { path: std::path::PathBuf },
    /// List installed plugins in the user plugin directory.
    Ls,
    /// Remove a plugin file from the user plugin directory.
    Remove { name: String },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LINSIGHT_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let cli = Cli::parse();

    // `db` subcommands are offline-only and never connect to the daemon.
    // Resolve the socket lazily so `linsight-cli db stats` works even when
    // $XDG_RUNTIME_DIR is unset.
    if let Cmd::Db { action } = cli.command {
        return match action {
            DbCmd::Stats { db } => commands::db::stats(db),
            DbCmd::Prune { older_than, vacuum, db } => commands::db::prune(db, &older_than, vacuum),
            DbCmd::Export { format, sensor, since, db, output } => {
                commands::db::export(db, sensor.as_deref(), &since, &format, output.as_deref())
            }
        };
    }

    let socket = match cli.socket {
        Some(socket) => socket,
        None => default_socket_path()?,
    };
    match cli.command {
        Cmd::List => commands::list::run(&socket),
        Cmd::Read { sensor, count } => commands::read::run(&socket, &sensor, count),
        Cmd::Watch { sensors, rate, format, count } => {
            commands::watch::run(&socket, &sensors, rate, &format, count)
        }
        Cmd::Alert { action } => match action {
            AlertCmd::List => commands::alert::list(&socket),
            AlertCmd::Add { name, expr, for_duration, cooldown, notify } => commands::alert::add(
                &socket,
                &name,
                &expr,
                for_duration.as_deref(),
                cooldown.as_deref(),
                &notify,
            ),
            AlertCmd::Remove { name } => commands::alert::remove(&socket, &name),
        },
        Cmd::History { sensor, last, format, max_points } => {
            commands::history::run(&socket, &sensor, &last, &format, max_points)
        }
        Cmd::Db { .. } => unreachable!("handled above"),
        Cmd::Plugin { action } => match action {
            PluginCmd::New { name } => commands::plugin::new(&name),
            PluginCmd::Install { path } => commands::plugin::install(&path),
            PluginCmd::Ls => commands::plugin::ls(),
            PluginCmd::Remove { name } => commands::plugin::remove(&name),
        },
    }
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
