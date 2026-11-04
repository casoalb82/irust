use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use tokio_rustls::rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use tokio_rustls::rustls::{ClientConfig, DigitallySignedStruct, Error, RootCertStore, SignatureScheme};
use tokio_rustls::TlsConnector;

pub fn connector(client_auth: Option<(&Path, &Path)>) -> anyhow::Result<TlsConnector> {
    let mut roots = RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        // A handful of malformed/duplicate system certs are common and
        // harmless to skip; only bail if the whole load produced nothing.
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        anyhow::bail!("no trusted root certificates found in the system store");
    }

    let builder = ClientConfig::builder().with_root_certificates(roots);
    let config = match client_auth {
        Some((cert_path, key_path)) => {
            builder.with_client_auth_cert(load_cert_chain(cert_path)?, load_private_key(key_path)?)?
        }
        None => builder.with_no_client_auth(),
    };

    Ok(TlsConnector::from(Arc::new(config)))
}

pub fn connector_insecure(client_auth: Option<(&Path, &Path)>) -> anyhow::Result<TlsConnector> {
    let builder = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify));
    let config = match client_auth {
        Some((cert_path, key_path)) => {
            builder.with_client_auth_cert(load_cert_chain(cert_path)?, load_private_key(key_path)?)?
        }
        None => builder.with_no_client_auth(),
    };

    Ok(TlsConnector::from(Arc::new(config)))
}

pub fn connector_pinned(
    expected_fingerprint_hex: &str,
    client_auth: Option<(&Path, &Path)>,
) -> anyhow::Result<TlsConnector> {
    let expected = parse_fingerprint_hex(expected_fingerprint_hex)?;
    let builder = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedVerifier { expected }));
    let config = match client_auth {
        Some((cert_path, key_path)) => {
            builder.with_client_auth_cert(load_cert_chain(cert_path)?, load_private_key(key_path)?)?
        }
        None => builder.with_no_client_auth(),
    };
    Ok(TlsConnector::from(Arc::new(config)))
}

fn parse_fingerprint_hex(raw: &str) -> anyhow::Result<[u8; 32]> {
    let cleaned: String = raw.chars().filter(|c| *c != ':' && !c.is_whitespace()).collect();
    let bytes = hex_decode(&cleaned)?;
    bytes
        .try_into()
        .map_err(|v: Vec<u8>| anyhow::anyhow!("expected a 32-byte SHA-256 fingerprint, got {} bytes", v.len()))
}

fn hex_decode(s: &str) -> anyhow::Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        anyhow::bail!("fingerprint has an odd number of hex digits");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| anyhow::anyhow!("invalid hex in fingerprint: {e}")))
        .collect()
}

#[derive(Debug)]
struct PinnedVerifier {
    expected: [u8; 32],
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let actual: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if actual == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(Error::General(format!(
                "certificate fingerprint mismatch: expected {}, got {}",
                hex_encode(&self.expected),
                hex_encode(&actual)
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &CryptoProvider::get_default()
                .expect("a default crypto provider is installed")
                .signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &CryptoProvider::get_default()
                .expect("a default crypto provider is installed")
                .signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        CryptoProvider::get_default()
            .expect("a default crypto provider is installed")
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn load_cert_chain(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let mut reader = std::io::BufReader::new(
        std::fs::File::open(path).map_err(|e| anyhow::anyhow!("opening client cert {}: {e}", path.display()))?,
    );
    let certs: Vec<_> = rustls_pemfile::certs(&mut reader).collect::<Result<_, _>>()?;
    if certs.is_empty() {
        anyhow::bail!("no certificates found in {}", path.display());
    }
    Ok(certs)
}

fn load_private_key(path: &Path) -> anyhow::Result<PrivateKeyDer<'static>> {
    let mut reader = std::io::BufReader::new(
        std::fs::File::open(path).map_err(|e| anyhow::anyhow!("opening client key {}: {e}", path.display()))?,
    );
    rustls_pemfile::private_key(&mut reader)?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", path.display()))
}

#[derive(Debug)]
struct NoVerify;

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &CryptoProvider::get_default()
                .expect("a default crypto provider is installed")
                .signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &CryptoProvider::get_default()
                .expect("a default crypto provider is installed")
                .signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        CryptoProvider::get_default()
            .expect("a default crypto provider is installed")
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod pinned_tests {
    use super::*;

    #[test]
    fn parses_plain_and_colon_separated_hex_the_same_way() {
        // Real fingerprints are 32 bytes (64 hex digits) -- build one
        // programmatically rather than hand-typing 64 digits.
        let expected: [u8; 32] = std::array::from_fn(|i| i as u8);
        let hex_plain: String = expected.iter().map(|b| format!("{b:02x}")).collect();
        // Same bytes, colon-separated (irssi's/openssl's own display
        // format), uppercase -- both forms real tools actually show.
        let hex_colon: String = expected
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");

        assert_eq!(parse_fingerprint_hex(&hex_plain).unwrap(), expected);
        assert_eq!(parse_fingerprint_hex(&hex_colon).unwrap(), expected);
    }

    #[test]
    fn rejects_the_wrong_length() {
        assert!(parse_fingerprint_hex("aabbcc").is_err());
    }

    #[test]
    fn rejects_non_hex_characters() {
        let bad = "zz".repeat(32);
        assert!(parse_fingerprint_hex(&bad).is_err());
    }

    #[test]
    fn verifier_accepts_a_matching_cert_and_rejects_a_different_one() {
        let cert_a = CertificateDer::from(vec![1, 2, 3, 4]);
        let cert_b = CertificateDer::from(vec![5, 6, 7, 8]);
        let expected: [u8; 32] = Sha256::digest(cert_a.as_ref()).into();
        let verifier = PinnedVerifier { expected };
        let server_name = ServerName::try_from("example.invalid").unwrap();

        assert!(verifier
            .verify_server_cert(&cert_a, &[], &server_name, &[], UnixTime::now())
            .is_ok());
        assert!(verifier
            .verify_server_cert(&cert_b, &[], &server_name, &[], UnixTime::now())
            .is_err());
    }
}
