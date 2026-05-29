// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![deny(rust_2018_idioms)]
#![deny(unsafe_op_in_unsafe_fn)]

//! `linsight-tunnel` exposes the LinSight daemon's Unix socket over a TCP+mTLS
//! transport so a remote desktop can talk to a remote `linsightd` directly,
//! without relying on SSH socket forwarding.
//!
//! The tunnel is protocol-agnostic: each accepted connection is paired with the
//! peer side and bytes are copied bidirectionally with `tokio::io::copy_bidirectional`.
//!
//! Two modes:
//!   * `server` — terminates mTLS on a TCP port, dials the local Unix socket.
//!   * `client` — listens on a local Unix socket, dials the remote mTLS server.
//!
//! Lifecycle: both modes install a Ctrl+C / SIGTERM handler. On signal the
//! accept loop exits, in-flight connections are given up to
//! [`DRAIN_TIMEOUT`] to finish their copy_bidirectional cleanly (which sends
//! TLS `close_notify`), and the process exits. Client mode also removes its
//! Unix socket on the way out via a `Drop` guard so the next start doesn't
//! trip the stale-socket guard.
//!
//! Concurrency: each mode caps the number of in-flight connections via a
//! semaphore (default 64). Excess connections are rejected immediately
//! before TLS auth is even attempted — important because the auth path
//! allocates and a misbehaving peer could otherwise pre-auth-DoS the daemon.

use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, ServerConfig};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::{error, info, warn};
use x509_parser::prelude::*;

// Default to 127.0.0.1 rather than 0.0.0.0 so the tunnel doesn't
// silently expose the daemon to every interface on the host. Operators
// who genuinely want a public bind pass `--bind 0.0.0.0:9443`
// explicitly; the commit message for the original tunnel feature
// already recommends "SSH forwarding remains the recommended path for
// most users", which is consistent with a localhost-only default.
const DEFAULT_SERVER_BIND: &str = "127.0.0.1:9443";
const DEFAULT_MAX_CONNECTIONS: usize = 64;
/// Time we wait after the shutdown signal for in-flight tunnels to finish
/// their bidirectional copy cleanly. Past this we abort outstanding tasks
/// so we don't hang the process forever on a wedged connection.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Parser, Debug)]
#[command(version, about = "LinSight mTLS remote tunnel")]
struct Cli {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand, Debug)]
enum Mode {
    /// Listen on TCP+TLS (mTLS) and forward each connection to the local LinSight Unix socket.
    Server(ServerArgs),
    /// Listen on a local Unix socket and forward each connection to a remote mTLS LinSight tunnel.
    Client(ClientArgs),
}

#[derive(Parser, Debug)]
struct ServerArgs {
    /// TCP address to bind for incoming TLS connections.
    #[arg(long, default_value = DEFAULT_SERVER_BIND)]
    bind: SocketAddr,
    /// Server certificate chain (PEM).
    #[arg(long)]
    cert: PathBuf,
    /// Server private key (PEM, PKCS#8 / RSA / SEC1).
    #[arg(long)]
    key: PathBuf,
    /// CA bundle used to verify client certificates (PEM).
    #[arg(long)]
    ca: PathBuf,
    /// Path to the local LinSight Unix socket to forward into.
    #[arg(long)]
    socket: PathBuf,
    /// Maximum concurrent connections. Excess connections are dropped
    /// before TLS auth so a connection burst cannot pre-auth DoS the
    /// daemon.
    #[arg(long, default_value_t = DEFAULT_MAX_CONNECTIONS)]
    max_connections: usize,
    /// Allow only client certificates whose Subject CommonName (CN)
    /// exactly matches one of the given values. Repeatable.
    /// If neither `--allow-cn` nor `--allow-san` is specified any
    /// CA-signed client certificate is accepted.
    #[arg(long)]
    allow_cn: Vec<String>,
    /// Allow only client certificates whose SubjectAltName DNS entry
    /// matches one of the given values. Wildcards (`*.example.com`)
    /// are supported as a prefix that matches any single DNS label.
    /// Repeatable.
    #[arg(long)]
    allow_san: Vec<String>,
}

#[derive(Parser, Debug)]
struct ClientArgs {
    /// Local Unix socket path to listen on (clients connect here as if it were `linsightd`).
    #[arg(long)]
    listen: PathBuf,
    /// Remote LinSight tunnel server, as `host:port`.
    #[arg(long)]
    server: String,
    /// Client certificate chain (PEM).
    #[arg(long)]
    cert: PathBuf,
    /// Client private key (PEM, PKCS#8 / RSA / SEC1).
    #[arg(long)]
    key: PathBuf,
    /// CA bundle used to verify the server's certificate (PEM).
    #[arg(long)]
    ca: PathBuf,
    /// Optional SNI / server name to send in TLS. Defaults to the hostname portion of `--server`.
    #[arg(long)]
    server_name: Option<String>,
    /// Maximum concurrent connections.
    #[arg(long, default_value_t = DEFAULT_MAX_CONNECTIONS)]
    max_connections: usize,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LINSIGHT_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // rustls 0.23 requires a process-wide default CryptoProvider.
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow!("failed to install rustls ring CryptoProvider"))?;

    let cli = Cli::parse();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;

    rt.block_on(async move {
        match cli.mode {
            Mode::Server(args) => run_server(args).await,
            Mode::Client(args) => run_client(args).await,
        }
    })
}

/// Future that resolves on first Ctrl+C OR SIGTERM. Sticky: after one
/// signal both branches resolve and the future stays resolved.
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };
    let sigterm = async {
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                // Without SIGTERM the process can still be stopped via
                // Ctrl+C; warn and park so the other branch wins.
                warn!(error = %e, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::select! {
        _ = ctrl_c => info!("Ctrl+C received; draining"),
        _ = sigterm => info!("SIGTERM received; draining"),
    }
}

/// Wait for `tasks` to drain or the deadline to elapse, whichever comes
/// first. Logs how many tasks didn't finish so the operator can tune the
/// timeout if needed.
async fn drain(mut tasks: JoinSet<()>, timeout: Duration) {
    let drained =
        tokio::time::timeout(timeout, async { while tasks.join_next().await.is_some() {} }).await;
    if drained.is_err() {
        let remaining = tasks.len();
        warn!(
            remaining,
            timeout_secs = timeout.as_secs(),
            "drain deadline elapsed with in-flight connections; aborting them",
        );
        tasks.abort_all();
    }
}

// ---------------------------------------------------------------------------
// PEM loading helpers
// ---------------------------------------------------------------------------

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let mut reader = BufReader::new(
        File::open(path).with_context(|| format!("opening cert {}", path.display()))?,
    );
    let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("parsing certs from {}", path.display()))?;
    if certs.is_empty() {
        bail!("no certificates found in {}", path.display());
    }
    Ok(certs)
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let mut reader = BufReader::new(
        File::open(path).with_context(|| format!("opening key {}", path.display()))?,
    );
    rustls_pemfile::private_key(&mut reader)
        .with_context(|| format!("parsing key from {}", path.display()))?
        .ok_or_else(|| anyhow!("no private key found in {}", path.display()))
}

fn load_roots(path: &Path) -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    let certs = load_certs(path)?;
    let (added, ignored) = roots.add_parsable_certificates(certs);
    if added == 0 {
        bail!("no usable CA certificates found in {}", path.display());
    }
    if ignored > 0 {
        warn!(ignored, "some CA certificates in {} were ignored", path.display());
    }
    Ok(roots)
}

// ---------------------------------------------------------------------------
// Client certificate CN / SAN allowlist checking
// ---------------------------------------------------------------------------

/// Extract the Subject CommonName (CN) from a DER-encoded X.509 certificate.
fn extract_cn(der: &CertificateDer<'_>) -> Option<String> {
    let (_, cert) = X509Certificate::from_der(der).ok()?;
    let name = cert.subject();
    name.iter_common_name().next().and_then(|cn| cn.as_str().ok()).map(|s| s.to_string())
}

/// Extract all DNS SubjectAltName entries from a DER-encoded X.509 certificate.
fn extract_dns_sans(der: &CertificateDer<'_>) -> Vec<String> {
    let (_, cert) = match X509Certificate::from_der(der).ok() {
        Some(v) => v,
        None => return Vec::new(),
    };
    match cert.subject_alternative_name() {
        Ok(Some(san_ext)) => san_ext
            .value
            .general_names
            .iter()
            .filter_map(|gn| match gn {
                GeneralName::DNSName(name) => Some(name.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Check whether `name` matches a pattern that may include a leading `*.`
/// wildcard. The wildcard must occupy the entire leftmost label and is the
/// only wildcard character supported — no mid-string `*`, no suffix `*`.
///
/// Examples:
///   `*.example.com` matches `foo.example.com` but NOT `foo.bar.example.com`
///   `example.com` matches only `example.com` (exact)
fn wildcard_match(pattern: &str, name: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix("*.") {
        // Single-label prefix wildcard: match the remaining suffix
        // exactly, but only against a single label after the first dot.
        match name.split_once('.') {
            Some((first, rest)) => !first.is_empty() && rest.eq_ignore_ascii_case(suffix),
            None => false,
        }
    } else {
        pattern.eq_ignore_ascii_case(name)
    }
}

/// A custom [`ClientCertVerifier`] that wraps an inner verifier (typically
/// a [`WebPkiClientVerifier`]) and adds optional CN / SAN allowlist
/// enforcement.
///
/// When no allowlists are configured this wrapper is a transparent
/// pass-through — any CA-signed client cert is accepted.
struct AllowlistClientCertVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    allow_cn: Vec<String>,
    allow_san: Vec<String>,
}

impl std::fmt::Debug for AllowlistClientCertVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AllowlistClientCertVerifier")
            .field("inner", &self.inner)
            .field("allow_cn", &self.allow_cn)
            .field("allow_san", &self.allow_san)
            .finish()
    }
}

impl ClientCertVerifier for AllowlistClientCertVerifier {
    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }

    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        // 1. First run the standard PKI chain validation (inner verifier).
        self.inner.verify_client_cert(end_entity, intermediates, now)?;

        // 2. If allowlists are specified, enforce them.
        if self.allow_cn.is_empty() && self.allow_san.is_empty() {
            return Ok(ClientCertVerified::assertion());
        }

        let cn = extract_cn(end_entity);
        let sans = extract_dns_sans(end_entity);

        // Check CN list.
        if !self.allow_cn.is_empty()
            && let Some(ref cn_str) = cn
            && self.allow_cn.iter().any(|p| wildcard_match(p, cn_str))
        {
            return Ok(ClientCertVerified::assertion());
        }

        // Check SAN DNS list.
        if !self.allow_san.is_empty() {
            for san in &sans {
                if self.allow_san.iter().any(|p| wildcard_match(p, san)) {
                    return Ok(ClientCertVerified::assertion());
                }
            }
        }

        // Nothing matched — reject.
        Err(rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }

    fn requires_raw_public_keys(&self) -> bool {
        self.inner.requires_raw_public_keys()
    }
}

// ---------------------------------------------------------------------------
// Server mode
// ---------------------------------------------------------------------------

async fn run_server(args: ServerArgs) -> Result<()> {
    let certs = load_certs(&args.cert)?;
    let key = load_key(&args.key)?;
    let client_roots = Arc::new(load_roots(&args.ca)?);

    // Build the standard WebPki client verifier that does PKI chain
    // validation against the configured CA bundle.
    let inner_verifier = WebPkiClientVerifier::builder(client_roots)
        .build()
        .context("building client cert verifier")?;

    // Wrap with CN/SAN allowlist enforcement if --allow-cn or
    // --allow-san were passed.  When neither is specified the wrapper
    // is a no-op pass-through (accept any CA-signed client cert).
    let has_filters = !args.allow_cn.is_empty() || !args.allow_san.is_empty();
    if has_filters {
        info!(
            allow_cn = ?args.allow_cn,
            allow_san = ?args.allow_san,
            "client certificate CN/SAN allowlist filtering enabled",
        );
    }
    let client_verifier: Arc<dyn ClientCertVerifier> = Arc::new(AllowlistClientCertVerifier {
        inner: inner_verifier,
        allow_cn: args.allow_cn,
        allow_san: args.allow_san,
    });

    let tls_config = ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(certs, key)
        .context("building TLS server config")?;
    let acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener =
        TcpListener::bind(args.bind).await.with_context(|| format!("binding TCP {}", args.bind))?;
    info!(
        bind = %args.bind,
        socket = %args.socket.display(),
        max_connections = args.max_connections,
        "linsight-tunnel server listening",
    );

    let socket = Arc::new(args.socket);
    let permits = Arc::new(Semaphore::new(args.max_connections));
    let mut tasks: JoinSet<()> = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            _ = shutdown_signal() => break,
            accepted = listener.accept() => {
                let (tcp, peer) = match accepted {
                    Ok(v) => v,
                    Err(e) => {
                        error!(error = %e, "accept failed");
                        continue;
                    }
                };
                let permit = match Arc::clone(&permits).try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        warn!(%peer, "rejecting: max-connections reached");
                        drop(tcp);
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let socket = Arc::clone(&socket);
                tasks.spawn(async move {
                    let _permit = permit;
                    if let Err(e) = handle_server_conn(acceptor, tcp, peer, &socket).await {
                        warn!(%peer, error = %e, "server connection ended with error");
                    }
                });
            }
        }
    }

    drain(tasks, DRAIN_TIMEOUT).await;
    Ok(())
}

async fn handle_server_conn(
    acceptor: TlsAcceptor,
    tcp: TcpStream,
    peer: SocketAddr,
    socket_path: &Path,
) -> Result<()> {
    tcp.set_nodelay(true).context("set TCP_NODELAY on inbound TLS connection")?;
    let mut tls =
        acceptor.accept(tcp).await.with_context(|| format!("TLS handshake with {peer}"))?;
    info!(%peer, "TLS client accepted");

    let mut unix = UnixStream::connect(socket_path).await.with_context(|| {
        format!(
            "connecting local LinSight daemon socket {} (is `linsightd` running?)",
            socket_path.display(),
        )
    })?;

    match copy_bidirectional(&mut tls, &mut unix).await {
        Ok((c2s, s2c)) => {
            info!(%peer, bytes_c2s = c2s, bytes_s2c = s2c, "tunnel closed");
            Ok(())
        }
        Err(e) => Err(anyhow!(e)).context("bidirectional copy failed"),
    }
}

// ---------------------------------------------------------------------------
// Client mode
// ---------------------------------------------------------------------------

/// RAII guard that removes the client-mode listener socket on drop. Ensures
/// a crashed or Ctrl+C'd client doesn't leave a stale socket file behind.
struct ListenSocketGuard(PathBuf);

impl Drop for ListenSocketGuard {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.0) {
            Ok(()) => info!(path = %self.0.display(), "removed client listen socket"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!(path = %self.0.display(), error = %e, "failed to remove client socket"),
        }
    }
}

async fn run_client(args: ClientArgs) -> Result<()> {
    let certs = load_certs(&args.cert)?;
    let key = load_key(&args.key)?;
    let server_roots = Arc::new(load_roots(&args.ca)?);

    let server_verifier = WebPkiServerVerifier::builder(server_roots)
        .build()
        .context("building server cert verifier")?;

    let tls_config = ClientConfig::builder()
        .with_webpki_verifier(server_verifier)
        .with_client_auth_cert(certs, key)
        .context("building TLS client config")?;
    let connector = TlsConnector::from(Arc::new(tls_config));

    let sni_host = match &args.server_name {
        Some(s) => s.clone(),
        None => args
            .server
            .rsplit_once(':')
            .map(|(h, _)| h.trim_start_matches('[').trim_end_matches(']').to_string())
            .ok_or_else(|| anyhow!("--server must be host:port"))?,
    };
    // Validate up-front that the SNI parses; clone per-connection later.
    ServerName::try_from(sni_host.as_str())
        .map_err(|e| anyhow!("invalid server name {sni_host:?}: {e}"))?;

    // Stale-socket cleanup: try connecting to the path first. If something
    // is actually listening, refuse to clobber it; if nothing answers, the
    // file is stale and removable. This avoids the previous TOCTOU race
    // between `exists()` and `remove_file()` plus protects against
    // accidentally killing a healthy peer's socket.
    if args.listen.exists() {
        match UnixStream::connect(&args.listen).await {
            Ok(_) => bail!(
                "{} is already in use by a live listener; refusing to overwrite",
                args.listen.display(),
            ),
            Err(_) => {
                std::fs::remove_file(&args.listen)
                    .with_context(|| format!("removing stale socket {}", args.listen.display()))?;
            }
        }
    }
    let listener = UnixListener::bind(&args.listen)
        .with_context(|| format!("binding unix socket {}", args.listen.display()))?;
    let _socket_guard = ListenSocketGuard(args.listen.clone());
    info!(
        listen = %args.listen.display(),
        server = %args.server,
        sni = %sni_host,
        max_connections = args.max_connections,
        "linsight-tunnel client listening",
    );

    let server_addr = Arc::new(args.server);
    let sni_host = Arc::new(sni_host);
    let permits = Arc::new(Semaphore::new(args.max_connections));
    let mut tasks: JoinSet<()> = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            _ = shutdown_signal() => break,
            accepted = listener.accept() => {
                let (unix, _) = match accepted {
                    Ok(v) => v,
                    Err(e) => {
                        error!(error = %e, "accept failed");
                        continue;
                    }
                };
                let permit = match Arc::clone(&permits).try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        warn!("rejecting: max-connections reached");
                        drop(unix);
                        continue;
                    }
                };
                let connector = connector.clone();
                let server_addr = Arc::clone(&server_addr);
                let sni_host = Arc::clone(&sni_host);
                tasks.spawn(async move {
                    let _permit = permit;
                    if let Err(e) = handle_client_conn(connector, unix, &server_addr, &sni_host).await {
                        warn!(error = %e, "client connection ended with error");
                    }
                });
            }
        }
    }

    drain(tasks, DRAIN_TIMEOUT).await;
    Ok(())
}

async fn handle_client_conn(
    connector: TlsConnector,
    mut unix: UnixStream,
    server_addr: &str,
    sni_host: &str,
) -> Result<()> {
    let tcp = TcpStream::connect(server_addr)
        .await
        .with_context(|| format!("connecting to {server_addr}"))?;
    tcp.set_nodelay(true).context("set TCP_NODELAY on outbound TLS connection")?;

    let sni = ServerName::try_from(sni_host.to_owned())
        .map_err(|e| anyhow!("invalid server name {sni_host:?}: {e}"))?;
    let mut tls = connector
        .connect(sni, tcp)
        .await
        .with_context(|| format!("TLS handshake with {server_addr}"))?;
    info!(server = %server_addr, "TLS connected upstream");

    match copy_bidirectional(&mut unix, &mut tls).await {
        Ok((c2s, s2c)) => {
            info!(bytes_c2s = c2s, bytes_s2c = s2c, "tunnel closed");
            Ok(())
        }
        Err(e) => Err(anyhow!(e)).context("bidirectional copy failed"),
    }
}
