// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! LinSight GUI shell entry point. cxx-qt 0.8 bridges Rust to Qt 6 / QML.
//! QObjects defined in `qobjects/` are auto-registered with QML under
//! `com.visorcraft.LinSight 1.0` by the `qml_module()` declaration in
//! `build.rs`.

mod client;
mod qobjects;
mod screenshot;
mod translator;
mod workspace;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{ArgAction, Parser};
use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QString, QUrl};
use tracing_subscriber::EnvFilter;

use crate::client::Client;
use crate::workspace::{Workspace, default_socket_path};

/// Default screenshot warm-up delay. Long enough for QML scene + sensor
/// catalogue to settle on a cold start; short enough not to feel like
/// the binary hung. Mirrored by `scripts/dev_screenshot.sh`.
const DEFAULT_SCREENSHOT_DELAY_MS: u32 = 2500;
/// Max screenshot warm-up. Past 30s the user almost certainly mis-typed.
const MAX_SCREENSHOT_DELAY_MS: u32 = 30_000;

#[derive(Parser, Debug)]
#[command(
    name = "linsight",
    version,
    about = "LinSight — multi-GPU Linux system-monitoring dashboard (GUI)",
    long_about = "Launches the Qt 6 / Kirigami GUI. If no daemon is listening on the local socket, \
                  one is auto-spawned as a child process. Pass `--connect ssh://user@host` to \
                  attach to a remote daemon over an SSH-forwarded socket instead. The first \
                  positional argument is the page to open (overview / gpus / storage / network / \
                  hardware / processes / editor / settings / about / licenses / credits)."
)]
struct Cli {
    /// Page to open at launch (overview, gpus, storage, network, hardware,
    /// processes, editor, settings, about, licenses, credits). Anything else is ignored.
    page: Option<String>,

    /// Attach to a remote daemon via SSH port forwarding. Format:
    /// `ssh://[user@]host[:port]`.
    #[arg(long, value_name = "URL")]
    connect: Option<String>,

    /// Render the first visible QQuickWindow to PNG and exit. Bypasses
    /// the compositor via `QQuickWindow::grabWindow()` so the captured
    /// frame is not subject to Wayland's stale-surface cache.
    #[arg(long, value_name = "PATH")]
    screenshot: Option<PathBuf>,

    /// How long to wait after launch before grabbing the screenshot.
    /// Lets the QML scene + sensor catalogue settle. Clamped to
    /// [0, 30000].
    #[arg(long, value_name = "MS", default_value_t = DEFAULT_SCREENSHOT_DELAY_MS)]
    screenshot_delay: u32,

    /// Flatten every QML animation duration to 0. Useful for vestibular-
    /// sensitive users and for capturing flicker-free screenshots.
    /// QML's `DesignTokens` reads the same flag from
    /// `Qt.application.arguments`; declaring it in clap here gives us
    /// `--help`, shell completion, and a single source of truth for
    /// the alias (`--no-animations`).
    #[arg(long, alias = "no-animations", action = ArgAction::SetTrue)]
    reduce_motion: bool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("LINSIGHT_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("LinSight GUI starting");

    let cli = Cli::parse();
    let remote_url = cli.connect;
    let screenshot_path = cli.screenshot;
    let screenshot_delay_ms = cli.screenshot_delay.min(MAX_SCREENSHOT_DELAY_MS);
    let _initial_page = cli.page; // QML reads this from Qt.application.arguments.
    let _reduce_motion = cli.reduce_motion; // QML reads it the same way.

    // Validate the screenshot destination BEFORE building the Qt app
    // and arming the timer. A buried qWarning from the C++ side is a
    // poor signal compared to an anyhow::bail right here.
    if let Some(p) = screenshot_path.as_deref() {
        validate_screenshot_path(p)?;
    }

    cxx_qt::init_crate!(cxx_qt_lib);
    cxx_qt::init_crate!(linsight);
    cxx_qt::init_qml_module!("com.visorcraft.LinSight");

    let (client, initial_target) = match remote_url {
        Some(url) => {
            tracing::info!(url = %url, "connecting to remote daemon over SSH");
            (Client::connect_ssh(&url)?, url)
        }
        None => {
            let socket = default_socket_path()?;
            (Client::connect_or_spawn(&socket)?, "local".to_string())
        }
    };
    qobjects::install_workspace(Arc::new(Workspace::new(client, &initial_target)?));

    let mut app = QGuiApplication::new();
    if app.is_null() {
        anyhow::bail!("could not construct QGuiApplication");
    }
    if let Some(mut app) = app.as_mut() {
        app.as_mut().set_application_name(&QString::from("LinSight"));
        app.as_mut().set_application_version(&QString::from(env!("CARGO_PKG_VERSION")));
        app.as_mut().set_organization_name(&QString::from("VisorCraft"));
        app.as_mut().set_organization_domain(&QString::from("visorcraft.com"));
    }
    cxx_qt_lib::QGuiApplication::set_desktop_file_name(&QString::from("com.visorcraft.LinSight"));

    if translator::install_system_translator() {
        tracing::info!("installed Qt translator for system locale");
    } else {
        tracing::debug!(
            "no .qm catalog matched system locale; falling back to English source strings"
        );
    }

    let mut engine = QQmlApplicationEngine::new();
    if engine.is_null() {
        anyhow::bail!("could not construct QQmlApplicationEngine");
    }
    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/com/visorcraft/LinSight/qml/Main.qml"));
    }

    if let Some(path) = screenshot_path.as_deref() {
        let delay_i32 = i32::try_from(screenshot_delay_ms).unwrap_or(i32::MAX);
        tracing::info!(
            path = %path.display(),
            delay_ms = delay_i32,
            "armed in-app screenshot — will grab first visible QQuickWindow and exit",
        );
        screenshot::arm(path, delay_i32);
    }

    if let Some(app) = app.as_mut() {
        let code = app.exec();
        if code != 0 {
            tracing::warn!("Qt event loop exited with code {code}");
        }
    }

    Ok(())
}

/// Refuse to launch with a screenshot destination we can already see is
/// going to fail: parent directory missing, path is an existing
/// directory, or the parent isn't writable. Catching these here means
/// the user gets an anyhow error before Qt even starts, rather than a
/// silent timer fire ending in a buried `qWarning`.
fn validate_screenshot_path(path: &Path) -> anyhow::Result<()> {
    if path.is_dir() {
        anyhow::bail!("screenshot path `{}` is a directory; pass a filename", path.display(),);
    }
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if !parent.exists() {
        anyhow::bail!("screenshot path's parent directory `{}` does not exist", parent.display(),);
    }
    // Probe writability by attempting to open-create-truncate, then
    // remove the placeholder so the C++ side can write the real PNG.
    match std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path) {
        Ok(_) => {
            let _ = std::fs::remove_file(path);
            Ok(())
        }
        Err(e) => anyhow::bail!("screenshot path `{}` is not writable: {e}", path.display(),),
    }
}
