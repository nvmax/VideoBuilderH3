//! Verified HTTPS for hosted LLM APIs (pack PR 314): the system store first, the built-in one only when verification fails, a
//! configured CA authoritative, and nothing but a certificate refusal ever retried.
//!
//! The tests run a local TLS server with a certificate made for the test, so a real handshake is refused or accepted. No network is used
//! and no key is kept in the repository. One test reaches a public site with a known expired certificate and is ignored by default.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use h3_llm::tls::{bundled_store, client_trusting_pem, send, send_with, system_store, HttpsError, Store, Trust};
use reqwest::Client;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

struct Server {
    port: u16,
    /// Handshakes the client accepted. A client that refuses the certificate leaves this at zero.
    handshakes: Arc<AtomicUsize>,
    /// The server's certificate, as the PEM a client can be told to trust.
    pem: Vec<u8>,
}

/// A TLS server for `localhost` that answers every request with `status` and "ok". `expired` gives it a certificate that ended in 2001.
async fn tls_server(status: u16, expired: bool) -> Server {
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    if expired {
        params.not_before = rcgen::date_time_ymd(2000, 1, 1);
        params.not_after = rcgen::date_time_ymd(2001, 1, 1);
    }
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    let pem = cert.pem().into_bytes();
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(cert.der().to_vec())], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())))
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handshakes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&handshakes);
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let (acceptor, counted) = (acceptor.clone(), Arc::clone(&counted));
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else { return };
                counted.fetch_add(1, Ordering::SeqCst);
                read_head(&mut tls).await;
                let reply = format!("HTTP/1.1 {status} Test\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
                let _ = tls.write_all(reply.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    Server { port, handshakes, pem }
}

async fn read_head<S: AsyncReadExt + Unpin>(stream: &mut S) {
    let (mut got, mut chunk) = (Vec::new(), [0u8; 2048]);
    while !got.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => got.extend_from_slice(&chunk[..n]),
        }
    }
}

fn store(trust: Trust, client: Client) -> Store {
    Store { trust, client }
}

fn trusting(pem: &[u8]) -> Client {
    client_trusting_pem(&[pem.to_vec()])
}

fn trusting_nothing() -> Client {
    client_trusting_pem(&[])
}

fn certificate_message(error: HttpsError) -> String {
    match error {
        HttpsError::Certificate(message) => message,
        HttpsError::Request(other) => panic!("expected a certificate refusal, got a request error: {other}"),
    }
}

#[tokio::test]
async fn a_server_no_store_trusts_is_refused_with_its_host_and_every_stores_answer() {
    let server = tls_server(200, false).await;
    let url = format!("https://localhost:{}/v1/chat?key=SECRET-KEY", server.port);
    let stores = [store(Trust::System, trusting_nothing()), store(Trust::Bundled, trusting_nothing())];
    let message = certificate_message(send_with(&stores, &|c| c.get(&url).header("x-api-key", "SECRET-KEY")).await.unwrap_err());
    assert!(message.starts_with("TLS certificate validation failed for localhost: "), "{message}");
    assert!(message.contains("does not trust"), "{message}");
    assert!(message.contains("Built-in CA result:"), "{message}");
    assert!(message.contains("antivirus HTTPS inspection") && message.contains("remains enabled"), "{message}");
    // A key can travel in the query or a header; neither may reach the error text.
    assert!(!message.contains("SECRET-KEY") && !message.contains("/v1/chat"), "{message}");
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 0, "no request data was sent to a server that was refused");
}

#[tokio::test]
async fn the_second_store_is_used_when_the_first_refuses_the_certificate() {
    let server = tls_server(200, false).await;
    let url = format!("https://localhost:{}/", server.port);
    let builds = AtomicUsize::new(0);
    let stores = [store(Trust::System, trusting_nothing()), store(Trust::Bundled, trusting(&server.pem))];
    let response = send_with(&stores, &|c| {
        builds.fetch_add(1, Ordering::SeqCst);
        c.get(&url)
    })
    .await
    .unwrap();
    assert_eq!(response.text().await.unwrap(), "ok");
    assert_eq!(builds.load(Ordering::SeqCst), 2, "tried the second store once");
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_store_that_works_is_the_only_one_used() {
    let server = tls_server(200, false).await;
    let url = format!("https://localhost:{}/", server.port);
    let builds = AtomicUsize::new(0);
    let stores = [store(Trust::System, trusting(&server.pem)), store(Trust::Bundled, trusting_nothing())];
    send_with(&stores, &|c| {
        builds.fetch_add(1, Ordering::SeqCst);
        c.get(&url)
    })
    .await
    .unwrap();
    assert_eq!(builds.load(Ordering::SeqCst), 1);
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_http_answer_is_returned_and_never_retried() {
    for status in [401u16, 429, 500] {
        let server = tls_server(status, false).await;
        let url = format!("https://localhost:{}/", server.port);
        let builds = AtomicUsize::new(0);
        let stores = [store(Trust::System, trusting(&server.pem)), store(Trust::Bundled, trusting(&server.pem))];
        let response = send_with(&stores, &|c| {
            builds.fetch_add(1, Ordering::SeqCst);
            c.post(&url).body("{}")
        })
        .await
        .unwrap();
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(builds.load(Ordering::SeqCst), 1, "status {status} was retried");
    }
}

#[tokio::test]
async fn a_connection_failure_is_not_a_certificate_failure_and_is_not_retried() {
    // A port nothing listens on.
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let url = format!("https://localhost:{closed}/");
    let builds = AtomicUsize::new(0);
    let stores = [store(Trust::System, trusting_nothing()), store(Trust::Bundled, trusting_nothing())];
    let error = send_with(&stores, &|c| {
        builds.fetch_add(1, Ordering::SeqCst);
        c.get(&url)
    })
    .await
    .unwrap_err();
    assert!(matches!(error, HttpsError::Request(_)), "{error}");
    assert_eq!(builds.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_configured_ca_is_authoritative() {
    let server = tls_server(200, false).await;
    let other = tls_server(200, false).await;
    let url = format!("https://localhost:{}/", server.port);

    // The certificate the configuration names is trusted.
    let named = [store(Trust::Configured, trusting(&server.pem))];
    assert_eq!(send_with(&named, &|c| c.get(&url)).await.unwrap().text().await.unwrap(), "ok");

    // Another one is refused, with nothing else tried and the configuration named in the error.
    let builds = AtomicUsize::new(0);
    let wrong = [store(Trust::Configured, trusting(&other.pem))];
    let message = certificate_message(
        send_with(&wrong, &|c| {
            builds.fetch_add(1, Ordering::SeqCst);
            c.get(&url)
        })
        .await
        .unwrap_err(),
    );
    assert!(message.contains("SSL_CERT_FILE / SSL_CERT_DIR was kept"), "{message}");
    assert!(!message.contains("Built-in CA result"), "{message}");
    assert_eq!(builds.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_request_that_is_not_https_is_sent_once_as_it_is() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let Ok((mut tcp, _)) = listener.accept().await else { return };
        read_head(&mut tcp).await;
        let _ = tcp.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nplain").await;
    });
    let builds = AtomicUsize::new(0);
    let stores = [store(Trust::System, trusting_nothing()), store(Trust::Bundled, trusting_nothing())];
    let url = format!("http://127.0.0.1:{port}/");
    let response = send_with(&stores, &|c| {
        builds.fetch_add(1, Ordering::SeqCst);
        c.get(&url)
    })
    .await
    .unwrap();
    assert_eq!(response.text().await.unwrap(), "plain");
    assert_eq!(builds.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_expired_certificate_is_called_expired() {
    // Trusted as a root, so the only thing wrong with it is its dates.
    let server = tls_server(200, true).await;
    let url = format!("https://localhost:{}/", server.port);
    let stores = [store(Trust::System, trusting(&server.pem)), store(Trust::Bundled, trusting(&server.pem))];
    let message = certificate_message(send_with(&stores, &|c| c.get(&url)).await.unwrap_err());
    assert!(message.contains("the certificate has expired"), "{message}");
    assert!(message.contains("must be renewed"), "{message}");
}

#[tokio::test]
async fn a_certificate_for_another_name_is_called_that() {
    // The certificate is for "localhost"; the address is the IP.
    let server = tls_server(200, false).await;
    let url = format!("https://127.0.0.1:{}/", server.port);
    let stores = [store(Trust::System, trusting(&server.pem))];
    let message = certificate_message(send_with(&stores, &|c| c.get(&url)).await.unwrap_err());
    assert!(message.contains("not valid for this host name"), "{message}");
    assert!(message.contains("TLS certificate validation failed for 127.0.0.1"), "{message}");
}

/// Against a public site whose certificate expired in 2015, through the real stores.
#[tokio::test]
#[ignore = "needs the internet"]
async fn a_real_expired_certificate_is_refused_by_both_real_stores() {
    let error = send(|c| c.get("https://expired.badssl.com/")).await.unwrap_err();
    let message = certificate_message(error);
    assert!(message.contains("expired.badssl.com"), "{message}");
    assert!(message.contains("the certificate has expired"), "{message}");
    assert!(message.contains("Built-in CA result:"), "{message}");
}

/// A real hosted API answers (a refused key is an HTTP answer, so the certificate was accepted), through each real store alone.
#[tokio::test]
#[ignore = "needs the internet"]
async fn a_real_hosted_api_is_reached_through_each_real_store() {
    for (name, one) in [("system", system_store()), ("built-in", bundled_store())] {
        let response = send_with(&[one], &|c| c.get("https://api.openai.com/v1/models")).await.unwrap_or_else(|e| panic!("{name} store: {e}"));
        assert_eq!(response.status().as_u16(), 401, "{name} store: no key was sent");
    }
}
