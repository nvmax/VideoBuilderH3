//! Verified HTTPS for hosted LLM APIs, with a separate built-in CA fallback. Port of the pack's `llm/http_tls.py` (pack PR 314).
//!
//! A request first verifies the server against the operating system's trusted certificates. When that verification fails, and only
//! then, it is tried once more against the root certificates built into the app, an independent store. Hostname checks and
//! certificate verification stay on in both. HTTP answers (rate limits, refused keys), timeouts and every other network failure are
//! never retried, and a connection that fails verification has sent no request data yet.
//!
//! `SSL_CERT_FILE` / `SSL_CERT_DIR` stay authoritative: when either is set, only the certificates they name are trusted and nothing
//! else is tried. If every store refuses the server, the error names the host and says what to check. It never carries the URL's
//! query or any header, because a key can travel in either. An expired server or proxy certificate still has to be renewed by its
//! operator; none of this makes an invalid certificate acceptable.
//!
//! Only the hosted APIs use this. LM Studio, the user's own server and ComfyUI keep their own rules.

use std::fmt;
use std::fs;
use std::sync::OnceLock;

use reqwest::{Certificate, Client, RequestBuilder, Response};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;
use rustls::CertificateError;

/// The set of trusted certificates a client verifies servers against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// The operating system's certificate store.
    System,
    /// The root certificates built into the app, independent of the system store.
    Bundled,
    /// The certificates `SSL_CERT_FILE` / `SSL_CERT_DIR` name. Authoritative: nothing else is tried.
    Configured,
}

/// A client that trusts one set of certificates.
#[derive(Clone)]
pub struct Store {
    pub trust: Trust,
    pub client: Client,
}

/// Why a hosted request could not be made.
#[derive(Debug)]
pub enum HttpsError {
    /// Every store refused the server's certificate. The text says what to check.
    Certificate(String),
    /// Anything else: the request could not be built or sent, or the connection failed for another reason.
    Request(reqwest::Error),
}

impl fmt::Display for HttpsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Certificate(message) => f.write_str(message),
            Self::Request(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for HttpsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Certificate(_) => None,
            Self::Request(error) => Some(error),
        }
    }
}

/// A client that trusts exactly these certificates and no others. A certificate rustls cannot use as a trust anchor is left out, because
/// reqwest would refuse to build the client for one bad certificate.
fn client_trusting(certificates: impl IntoIterator<Item = CertificateDer<'static>>) -> Client {
    let mut accepted = rustls::RootCertStore::empty();
    let mut builder = Client::builder().tls_built_in_root_certs(false);
    for der in certificates {
        if accepted.add(der.clone()).is_ok() {
            if let Ok(certificate) = Certificate::from_der(der.as_ref()) {
                builder = builder.add_root_certificate(certificate);
            }
        }
    }
    // A client with no trusted certificate refuses every server, which is right for an empty or unreadable configuration.
    builder.build().unwrap_or_else(|_| Client::builder().tls_built_in_root_certs(false).build().unwrap_or_default())
}

/// A client that trusts the certificates in these PEM files and no others.
pub fn client_trusting_pem(files: &[Vec<u8>]) -> Client {
    client_trusting(files.iter().flat_map(|pem| CertificateDer::pem_slice_iter(pem).filter_map(Result::ok)))
}

/// The store that trusts the operating system's certificates.
pub fn system_store() -> Store {
    // Errors for single certificates are skipped: a store with a few unreadable entries is still a store.
    Store { trust: Trust::System, client: client_trusting(rustls_native_certs::load_native_certs().certs) }
}

/// The store that trusts the root certificates built into the app.
pub fn bundled_store() -> Store {
    // reqwest's own roots: the Mozilla list compiled into the app.
    Store { trust: Trust::Bundled, client: Client::builder().build().unwrap_or_default() }
}

fn env_text(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// The PEM files `SSL_CERT_FILE` / `SSL_CERT_DIR` name, or `None` when neither is set.
fn configured_pems() -> Option<Vec<Vec<u8>>> {
    let (file, dir) = (env_text("SSL_CERT_FILE"), env_text("SSL_CERT_DIR"));
    if file.is_none() && dir.is_none() {
        return None;
    }
    let mut pems = Vec::new();
    if let Some(file) = file {
        pems.extend(fs::read(file).ok());
    }
    if let Some(dirs) = dir {
        for folder in std::env::split_paths(&dirs) {
            let Ok(entries) = fs::read_dir(folder) else { continue };
            pems.extend(entries.flatten().filter(|e| e.path().is_file()).filter_map(|e| fs::read(e.path()).ok()));
        }
    }
    Some(pems)
}

/// The stores a hosted request tries, in order: the configured one alone when `SSL_CERT_FILE` / `SSL_CERT_DIR` is set, else the system
/// store and then the built-in one.
fn default_stores() -> Vec<Store> {
    match configured_pems() {
        Some(pems) => vec![Store { trust: Trust::Configured, client: client_trusting_pem(&pems) }],
        None => vec![system_store(), bundled_store()],
    }
}

fn stores() -> &'static [Store] {
    static STORES: OnceLock<Vec<Store>> = OnceLock::new();
    STORES.get_or_init(default_stores)
}

/// What a refused certificate means, in words.
fn describe(problem: &CertificateError) -> String {
    match problem {
        CertificateError::Expired | CertificateError::ExpiredContext { .. } => "the certificate has expired".into(),
        CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => "the certificate is not valid yet".into(),
        CertificateError::UnknownIssuer => "the certificate was issued by an authority this store does not trust".into(),
        CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => "the certificate is not valid for this host name".into(),
        CertificateError::Revoked => "the certificate was revoked".into(),
        other => format!("the certificate was refused ({other})"),
    }
}

/// The rustls error inside `error`, looking through the `io::Error`s the connection layers wrap it in. `io::Error::source` skips the error
/// it wraps, so those have to be opened with `get_ref`.
fn rustls_error_in<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a rustls::Error> {
    let mut current = error;
    loop {
        if let Some(found) = current.downcast_ref::<rustls::Error>() {
            return Some(found);
        }
        current = current.downcast_ref::<std::io::Error>()?.get_ref()?;
    }
}

/// The reason a server's certificate was refused, when that is why the request failed.
fn certificate_failure(error: &reqwest::Error) -> Option<String> {
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = cause {
        if let Some(rustls::Error::InvalidCertificate(problem)) = rustls_error_in(current) {
            return Some(describe(problem));
        }
        cause = current.source();
    }
    None
}

/// The error for a server every store refused: its host, what each store said and what to check.
fn explain(host: &str, failures: &[(Trust, String)]) -> String {
    let first = failures.first().map_or("verification failed", |(_, reason)| reason.as_str());
    let configured = failures.iter().any(|(trust, _)| *trust == Trust::Configured);
    let mut details = if configured {
        "The configured SSL_CERT_FILE / SSL_CERT_DIR was kept; check its CA certificates.".to_string()
    } else {
        "The system and the built-in CA certificates were both tried. Update the app to refresh the built-in ones.".to_string()
    };
    for (trust, reason) in failures.iter().skip(1) {
        let label = if *trust == Trust::Bundled { "Built-in CA result" } else { "Other store result" };
        details.push_str(&format!(" {label}: {reason}."));
    }
    format!(
        "TLS certificate validation failed for {host}: {first}. {details} Check the computer's date and time and any VPN, proxy or antivirus HTTPS inspection. \
         An expired server or proxy certificate must be renewed. Certificate verification remains enabled."
    )
}

/// Send a request to a hosted API. `build` makes the request for the client it is given, and is called again for the next store only when
/// the server's certificate was refused. A request that is not HTTPS goes out once, as it is.
pub async fn send(build: impl Fn(&Client) -> RequestBuilder + Sync) -> Result<Response, HttpsError> {
    send_with(stores(), &build).await
}

/// [`send`] with the stores to try, in order.
pub async fn send_with(stores: &[Store], build: &(dyn Fn(&Client) -> RequestBuilder + Sync)) -> Result<Response, HttpsError> {
    let mut failures: Vec<(Trust, String)> = Vec::new();
    let mut host = String::from("the API server");
    for store in stores {
        let request = build(&store.client).build().map_err(HttpsError::Request)?;
        if request.url().scheme() != "https" {
            return store.client.execute(request).await.map_err(HttpsError::Request);
        }
        if let Some(name) = request.url().host_str() {
            host = name.to_string();
        }
        match store.client.execute(request).await {
            Ok(response) => return Ok(response),
            Err(error) => match certificate_failure(&error) {
                Some(reason) => failures.push((store.trust, reason)),
                None => return Err(HttpsError::Request(error)),
            },
        }
    }
    Err(HttpsError::Certificate(explain(&host, &failures)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_certificate_is_named_plainly() {
        assert_eq!(describe(&CertificateError::Expired), "the certificate has expired");
        assert_eq!(describe(&CertificateError::NotValidYet), "the certificate is not valid yet");
        assert!(describe(&CertificateError::UnknownIssuer).contains("does not trust"));
        assert!(describe(&CertificateError::NotValidForName).contains("host name"));
        assert!(describe(&CertificateError::BadSignature).starts_with("the certificate was refused"));
    }

    #[test]
    fn the_error_names_the_host_each_stores_answer_and_what_to_check() {
        let both = explain("api.openai.com", &[(Trust::System, "the certificate has expired".into()), (Trust::Bundled, "the certificate was issued by an authority this store does not trust".into())]);
        assert!(both.starts_with("TLS certificate validation failed for api.openai.com: the certificate has expired."), "{both}");
        assert!(both.contains("Built-in CA result: the certificate was issued by an authority this store does not trust."), "{both}");
        assert!(both.contains("date and time") && both.contains("antivirus HTTPS inspection") && both.contains("must be renewed"), "{both}");
        assert!(both.ends_with("Certificate verification remains enabled."), "{both}");

        let configured = explain("example.test", &[(Trust::Configured, "the certificate has expired".into())]);
        assert!(configured.contains("SSL_CERT_FILE / SSL_CERT_DIR was kept"), "{configured}");
        assert!(!configured.contains("Built-in CA result"), "{configured}");
    }
}
