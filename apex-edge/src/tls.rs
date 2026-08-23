//! TLS/mTLS listener configuration for the ApexEdge hub.
//!
//! Plain HTTP remains the default so local dev and CI keep working without certs;
//! setting `APEX_EDGE_TLS_CERT_PATH` + `APEX_EDGE_TLS_KEY_PATH` switches the listener to
//! HTTPS, and additionally setting `APEX_EDGE_TLS_CLIENT_CA_PATH` requires clients to
//! present a certificate signed by that CA (mTLS).

use apex_edge_metrics::TLS_ENABLED;
use axum_server::tls_rustls::RustlsConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use std::sync::Arc;

pub struct TlsSettings {
    pub cert_path: String,
    pub key_path: String,
    pub client_ca_path: Option<String>,
}

impl TlsSettings {
    /// `None` when TLS env vars are unset, meaning the caller should serve plain HTTP.
    pub fn from_env() -> Option<Self> {
        let cert_path = std::env::var("APEX_EDGE_TLS_CERT_PATH").ok()?;
        let key_path = std::env::var("APEX_EDGE_TLS_KEY_PATH").ok()?;
        let client_ca_path = std::env::var("APEX_EDGE_TLS_CLIENT_CA_PATH").ok();
        Some(Self {
            cert_path,
            key_path,
            client_ca_path,
        })
    }

    fn client_auth_label(&self) -> &'static str {
        if self.client_ca_path.is_some() {
            "required"
        } else {
            "off"
        }
    }
}

/// Emit the TLS gauge for the current mode (called once at startup, whichever branch runs).
pub fn report_tls_enabled(settings: Option<&TlsSettings>) {
    match settings {
        Some(settings) => {
            metrics::gauge!(TLS_ENABLED, "client_auth" => settings.client_auth_label()).set(1.0);
        }
        None => {
            metrics::gauge!(TLS_ENABLED, "client_auth" => "off").set(0.0);
        }
    }
}

/// Build the rustls server config for `axum-server`, loading a plain cert/key pair or, when
/// a client CA is configured, a config that requires and verifies client certificates.
pub async fn build_rustls_config(settings: &TlsSettings) -> std::io::Result<RustlsConfig> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    match &settings.client_ca_path {
        None => RustlsConfig::from_pem_file(&settings.cert_path, &settings.key_path).await,
        Some(client_ca_path) => {
            let cert_path = settings.cert_path.clone();
            let key_path = settings.key_path.clone();
            let client_ca_path = client_ca_path.clone();
            let server_config = tokio::task::spawn_blocking(move || {
                build_mtls_server_config(&cert_path, &key_path, &client_ca_path)
            })
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))??;
            Ok(RustlsConfig::from_config(Arc::new(server_config)))
        }
    }
}

fn build_mtls_server_config(
    cert_path: &str,
    key_path: &str,
    client_ca_path: &str,
) -> std::io::Result<ServerConfig> {
    let certs = load_certs(cert_path)?;
    let key = load_key(key_path)?;

    let mut roots = RootCertStore::empty();
    for ca_cert in load_certs(client_ca_path)? {
        roots
            .add(ca_cert)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    }
    let client_verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    let mut config = ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(certs, key)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

fn load_certs(path: &str) -> std::io::Result<Vec<CertificateDer<'static>>> {
    CertificateDer::pem_file_iter(path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

fn load_key(path: &str) -> std::io::Result<PrivateKeyDer<'static>> {
    PrivateKeyDer::from_pem_file(path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_auth_label_reflects_client_ca_presence() {
        let plain = TlsSettings {
            cert_path: "cert.pem".into(),
            key_path: "key.pem".into(),
            client_ca_path: None,
        };
        assert_eq!(plain.client_auth_label(), "off");

        let mtls = TlsSettings {
            cert_path: "cert.pem".into(),
            key_path: "key.pem".into(),
            client_ca_path: Some("ca.pem".into()),
        };
        assert_eq!(mtls.client_auth_label(), "required");
    }

    #[tokio::test]
    async fn build_rustls_config_reports_missing_cert_file() {
        let settings = TlsSettings {
            cert_path: "does-not-exist-cert.pem".into(),
            key_path: "does-not-exist-key.pem".into(),
            client_ca_path: None,
        };
        assert!(build_rustls_config(&settings).await.is_err());
    }

    #[tokio::test]
    async fn mtls_config_reports_missing_client_ca_file() {
        let settings = TlsSettings {
            cert_path: "does-not-exist-cert.pem".into(),
            key_path: "does-not-exist-key.pem".into(),
            client_ca_path: Some("does-not-exist-ca.pem".into()),
        };
        assert!(build_rustls_config(&settings).await.is_err());
    }
}
