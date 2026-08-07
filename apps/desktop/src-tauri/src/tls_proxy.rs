//! Local TLS termination proxy (Hermes Desktop).
//!
//! WebView2 auto-upgrades outbound `ws://` / `http://` requests from the
//! secure `tauri.localhost` page to TLS (Chromium's Mixed Content
//! Autoupgrade), and the plain-HTTP `hermes serve` backend cannot answer a
//! TLS handshake — the renderer's WebSocket connection dies with
//! `ERR_SSL_PROTOCOL_ERROR` before any bytes reach the backend.
//!
//! This module listens on a random loopback port with a self-signed
//! certificate, terminates TLS locally (the renderer connects with
//! `wss://`/`https://`; `--ignore-certificate-errors` in tauri.conf.json
//! accepts the self-signed cert), and forwards the decrypted byte stream to
//! the plaintext backend port. Both WebSocket (long-lived) and plain HTTP
//! connections are transparently proxied via `copy_bidirectional`.

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::net::TcpListener;
use std::sync::Arc;

/// Start a TLS termination proxy on a random loopback port that forwards to
/// `backend_port`. Returns the proxy's port number.
pub fn spawn_tls_proxy(backend_port: u16) -> Result<u16, String> {
    // rustls (0.23) needs an explicit crypto provider when the `ring` feature
    // is enabled — install it once for the process.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("proxy bind: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("proxy local addr: {e}"))?
        .port();

    // Self-signed certificate covering the loopback names the renderer uses.
    let cert = rcgen::generate_simple_self_signed(vec![
        "127.0.0.1".to_string(),
        "localhost".to_string(),
        "::1".to_string(),
    ])
    .map_err(|e| format!("proxy cert generation: {e}"))?;
    let cert_der = CertificateDer::from(cert.cert.der().clone());
    let key_der = PrivateKeyDer::try_from(cert.key_pair.serialize_der())
        .map_err(|e| format!("proxy key parse: {e}"))?;
    let tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .map_err(|e| format!("proxy tls config: {e}"))?;
    let acceptor = Arc::new(tokio_rustls::TlsAcceptor::from(Arc::new(tls_config)));

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .expect("tls proxy runtime");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener)
                .expect("tls proxy listener into tokio");
            loop {
                let (client, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => continue,
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let mut tls = match acceptor.accept(client).await {
                        Ok(tls) => tls,
                        Err(_) => return, // TLS handshake failed; drop silently.
                    };
                    let mut backend = match tokio::net::TcpStream::connect(("127.0.0.1", backend_port))
                        .await
                    {
                        Ok(b) => b,
                        Err(_) => return,
                    };
                    let _ = tokio::io::copy_bidirectional(&mut tls, &mut backend).await;
                });
            }
        });
    });

    Ok(port)
}
