// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! End-to-end mTLS smoke test for the tunnel's TLS dep stack.
//!
//! Closes the open follow-up: "Live mTLS handshake smoke test for
//! linsight-tunnel — the binary compiles + --help works; an end-to-end test
//! with a generated dev cert pair hasn't run."
//!
//! What this exercises:
//!  * rcgen produces a self-signed CA + server + client cert chain that
//!    round-trips through `rustls_pemfile`.
//!  * `WebPkiServerVerifier` + `WebPkiClientVerifier` (ring provider, TLS
//!    1.2 + 1.3) successfully complete a mutual handshake with the chain
//!    above.
//!  * Bytes flow in both directions after the handshake — i.e. a working
//!    `copy_bidirectional`-shaped pipe is possible end-to-end.
//!  * An UNTRUSTED client cert (signed by a different CA) is rejected,
//!    proving the server verifier isn't accidentally accepting anything.

use std::io::Cursor;
use std::sync::Arc;

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

struct GeneratedCert {
    cert_pem: String,
    key_pem: String,
}

struct Pki {
    ca: GeneratedCert,
    server: GeneratedCert,
    client: GeneratedCert,
    /// A second CA + a client signed by it. Used to prove that a client
    /// from outside the trusted bundle is rejected, not accepted.
    rogue_client: GeneratedCert,
}

fn install_default_provider() {
    // rustls 0.23 needs a process-wide default. Idempotent — tests run
    // in the same process so the second install is a no-op.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn make_pki() -> Pki {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(vec![]).unwrap();
    ca_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "LinSight Test CA");
        dn
    };
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();

    let issue = |cn: &str, sans: Vec<String>| -> GeneratedCert {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(sans).unwrap();
        params.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, cn);
            dn
        };
        let cert = params.signed_by(&key, &ca_cert, &ca_key).unwrap();
        GeneratedCert { cert_pem: cert.pem(), key_pem: key.serialize_pem() }
    };

    let server = issue("linsight-test-server", vec!["linsight.test".into()]);
    let client = issue("linsight-test-client", vec![]);

    // Rogue CA + a client signed by it. The legitimate server verifier
    // doesn't trust this CA.
    let rogue_ca_key = KeyPair::generate().unwrap();
    let mut rogue_ca_params = CertificateParams::new(vec![]).unwrap();
    rogue_ca_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "Rogue CA");
        dn
    };
    rogue_ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let rogue_ca_cert = rogue_ca_params.self_signed(&rogue_ca_key).unwrap();
    let rogue_client_key = KeyPair::generate().unwrap();
    let mut rogue_client_params = CertificateParams::new(vec![]).unwrap();
    rogue_client_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "rogue-client");
        dn
    };
    let rogue_client_cert =
        rogue_client_params.signed_by(&rogue_client_key, &rogue_ca_cert, &rogue_ca_key).unwrap();
    let rogue_client = GeneratedCert {
        cert_pem: rogue_client_cert.pem(),
        key_pem: rogue_client_key.serialize_pem(),
    };

    Pki {
        ca: GeneratedCert { cert_pem: ca_cert.pem(), key_pem: ca_key.serialize_pem() },
        server,
        client,
        rogue_client,
    }
}

fn parse_chain(pem: &str) -> Vec<CertificateDer<'static>> {
    rustls_pemfile::certs(&mut Cursor::new(pem.as_bytes()))
        .collect::<std::io::Result<Vec<_>>>()
        .expect("parse cert chain")
}

fn parse_key(pem: &str) -> PrivateKeyDer<'static> {
    rustls_pemfile::private_key(&mut Cursor::new(pem.as_bytes()))
        .expect("read key pem")
        .expect("no key in pem")
}

fn build_server_config(server: &GeneratedCert, ca: &GeneratedCert) -> Arc<ServerConfig> {
    let mut roots = RootCertStore::empty();
    for c in parse_chain(&ca.cert_pem) {
        roots.add(c).expect("add ca to roots");
    }
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots)).build().expect("client verifier");
    Arc::new(
        ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(parse_chain(&server.cert_pem), parse_key(&server.key_pem))
            .expect("server config"),
    )
}

fn build_client_config(client: &GeneratedCert, ca: &GeneratedCert) -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    for c in parse_chain(&ca.cert_pem) {
        roots.add(c).expect("add ca to roots");
    }
    let verifier = WebPkiServerVerifier::builder(Arc::new(roots)).build().expect("server verifier");
    Arc::new(
        ClientConfig::builder()
            .with_webpki_verifier(verifier)
            .with_client_auth_cert(parse_chain(&client.cert_pem), parse_key(&client.key_pem))
            .expect("client config"),
    )
}

#[tokio::test]
async fn mtls_handshake_and_byte_round_trip() {
    install_default_provider();
    let pki = make_pki();

    let server_cfg = build_server_config(&pki.server, &pki.ca);
    let client_cfg = build_client_config(&pki.client, &pki.ca);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind server");
    let addr = listener.local_addr().expect("local addr");

    // Server side: accept one TLS connection and echo any bytes back.
    let server_task = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.expect("accept");
        let acceptor = TlsAcceptor::from(server_cfg);
        let mut tls = acceptor.accept(sock).await.expect("server tls accept");
        let mut buf = [0u8; 32];
        let n = tls.read(&mut buf).await.expect("server read");
        tls.write_all(&buf[..n]).await.expect("server write");
        tls.shutdown().await.expect("server shutdown");
    });

    // Client side: dial, send "hello", expect it back.
    let tcp = TcpStream::connect(addr).await.expect("dial");
    let connector = TlsConnector::from(client_cfg);
    let sni = ServerName::try_from("linsight.test").expect("sni");
    let mut tls = connector.connect(sni, tcp).await.expect("client tls connect");
    tls.write_all(b"hello mtls").await.expect("client write");
    let mut buf = vec![0u8; b"hello mtls".len()];
    tls.read_exact(&mut buf).await.expect("client read");
    assert_eq!(&buf, b"hello mtls");
    tls.shutdown().await.expect("client shutdown");

    server_task.await.expect("server task panicked");
}

#[tokio::test]
async fn mtls_rejects_untrusted_client_cert() {
    install_default_provider();
    let pki = make_pki();

    let server_cfg = build_server_config(&pki.server, &pki.ca);
    // Use the rogue client cert — signed by a CA the server does NOT trust.
    let client_cfg = build_client_config(&pki.rogue_client, &pki.ca);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind server");
    let addr = listener.local_addr().expect("local addr");

    let server_task = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.expect("accept");
        let acceptor = TlsAcceptor::from(server_cfg);
        // The handshake should fail; we propagate the error so the test
        // can observe it.
        acceptor.accept(sock).await.map(|_| ())
    });

    // TLS 1.3 lets the client see a successful Finished before the
    // server has fully validated the client cert — the server's
    // rejection arrives as a post-handshake alert that only surfaces
    // when we try to read or write. So the assertion below checks:
    // either connect() fails, OR the subsequent read/write fails. Both
    // are acceptable "the rogue cert was rejected" outcomes.
    let tcp = TcpStream::connect(addr).await.expect("dial");
    let connector = TlsConnector::from(client_cfg);
    let sni = ServerName::try_from("linsight.test").expect("sni");
    let connect_result = connector.connect(sni, tcp).await;
    let post_handshake_error = match connect_result {
        Err(_) => true,
        Ok(mut tls) => {
            // Server should send a fatal alert; either the write or
            // the subsequent read fails. Use a short timeout so a
            // wedged server doesn't hang the test forever.
            let attempt = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                tls.write_all(b"hello").await?;
                let mut buf = [0u8; 1];
                tls.read_exact(&mut buf).await
            })
            .await;
            // Timeout → wedged is also "rejected" — server never got
            // to the point of replying. write or read returning an
            // error is also "rejected".
            match attempt {
                Err(_) => true, // timeout
                Ok(Err(_)) => true,
                Ok(Ok(_)) => false,
            }
        }
    };
    assert!(
        post_handshake_error,
        "rogue client cert should have been rejected by the server verifier",
    );
    // Server task should have errored too.
    let server_result = server_task.await.expect("server task panicked");
    assert!(
        server_result.is_err(),
        "server-side handshake should have errored on rogue client cert; got {server_result:?}",
    );
}
