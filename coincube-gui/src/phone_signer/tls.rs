//! rustls configuration for the local LAN signer.
//!
//! Both sides hold a long-lived self-signed cert + Ed25519 keypair.
//! Trust is **pinned**: a pairing QR carries the SHA-256 of the
//! desktop's cert DER and the phone records the inverse on
//! `PairingComplete`. After pairing, each TLS handshake just checks
//! the peer's end-entity cert hashes to the pinned value — no PKI,
//! no CA, no hostname check.
//!
//! Both ClientConfig and ServerConfig use a [`PinnedVerifier`]; the
//! same struct implements [`ServerCertVerifier`] and
//! [`ClientCertVerifier`] so we don't repeat the comparison logic.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    ClientConfig, DigitallySignedStruct, DistinguishedName, Error as TlsError, ServerConfig,
    SignatureScheme,
};
use sha2::{Digest, Sha256};

/// SHA-256 of the peer's end-entity cert DER. 32 bytes; we compare
/// against this on every handshake.
pub type CertFingerprint = [u8; 32];

/// Compute the cert pin (SHA-256 of the DER bytes).
pub fn fingerprint_of(cert_der: &CertificateDer<'_>) -> CertFingerprint {
    let digest = Sha256::digest(cert_der.as_ref());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Verifier that accepts exactly one end-entity cert: the one whose
/// SHA-256 matches `expected`. Used in both directions.
#[derive(Debug)]
pub struct PinnedVerifier {
    expected: CertFingerprint,
    crypto: Arc<CryptoProvider>,
    no_dn: Vec<DistinguishedName>,
}

impl PinnedVerifier {
    pub fn new(expected: CertFingerprint) -> Arc<Self> {
        Arc::new(Self {
            expected,
            crypto: Arc::new(rustls::crypto::ring::default_provider()),
            no_dn: Vec::new(),
        })
    }

    fn verify_cert_pin(&self, end_entity: &CertificateDer<'_>) -> Result<(), TlsError> {
        let actual = fingerprint_of(end_entity);
        if actual == self.expected {
            Ok(())
        } else {
            // Use Other on TlsError so we don't pretend to be a
            // standard PKI failure — pin mismatch isn't really an
            // expired-cert situation.
            Err(TlsError::General("cert pin mismatch".into()))
        }
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        self.verify_cert_pin(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.crypto.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.crypto.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.crypto
            .signature_verification_algorithms
            .supported_schemes()
    }
}

impl ClientCertVerifier for PinnedVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &self.no_dn
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        self.verify_cert_pin(end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.crypto.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.crypto.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.crypto
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};
    use std::convert::TryFrom;
    use std::time::Duration;

    fn mint_cert() -> CertificateDer<'static> {
        let kp = KeyPair::generate_for(&PKCS_ED25519).expect("ed25519 keygen");
        let params = CertificateParams::new(vec!["test.local".to_string()]).expect("params");
        let cert = params.self_signed(&kp).expect("self-sign");
        cert.der().clone()
    }

    fn unix_now() -> UnixTime {
        UnixTime::since_unix_epoch(Duration::from_secs(0))
    }

    fn sni() -> ServerName<'static> {
        ServerName::try_from("test.local".to_string()).expect("sni")
    }

    #[test]
    fn fingerprint_of_matches_raw_sha256() {
        let cert = mint_cert();
        let pin = fingerprint_of(&cert);
        let expected: [u8; 32] = Sha256::digest(cert.as_ref()).into();
        assert_eq!(pin, expected);
    }

    #[test]
    fn pinned_verifier_accepts_matching_server_cert() {
        let cert = mint_cert();
        let pin = fingerprint_of(&cert);
        let v = PinnedVerifier::new(pin);
        let res =
            ServerCertVerifier::verify_server_cert(&*v, &cert, &[], &sni(), &[], unix_now());
        assert!(res.is_ok(), "matching pin should verify: {:?}", res.err());
    }

    #[test]
    fn pinned_verifier_rejects_mismatched_server_cert() {
        let cert = mint_cert();
        let other = mint_cert();
        let pin = fingerprint_of(&cert);
        let v = PinnedVerifier::new(pin);
        let res =
            ServerCertVerifier::verify_server_cert(&*v, &other, &[], &sni(), &[], unix_now());
        assert!(res.is_err(), "non-matching pin must reject");
    }

    #[test]
    fn pinned_verifier_accepts_matching_client_cert() {
        let cert = mint_cert();
        let pin = fingerprint_of(&cert);
        let v = PinnedVerifier::new(pin);
        let res = ClientCertVerifier::verify_client_cert(&*v, &cert, &[], unix_now());
        assert!(res.is_ok(), "matching pin should verify (client direction)");
    }

    #[test]
    fn pinned_verifier_rejects_mismatched_client_cert() {
        let cert = mint_cert();
        let other = mint_cert();
        let pin = fingerprint_of(&cert);
        let v = PinnedVerifier::new(pin);
        let res = ClientCertVerifier::verify_client_cert(&*v, &other, &[], unix_now());
        assert!(res.is_err(), "non-matching pin must reject (client direction)");
    }
}

/// Build a [`ClientConfig`] for dialling a paired phone, pinning the
/// phone's cert and presenting the desktop's own cert.
pub fn client_config(
    desktop_cert: CertificateDer<'static>,
    desktop_key: PrivateKeyDer<'static>,
    phone_cert_pin: CertFingerprint,
) -> Result<ClientConfig, TlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(PinnedVerifier::new(phone_cert_pin))
        .with_client_auth_cert(vec![desktop_cert], desktop_key)?;
    Ok(cfg)
}

/// Build a [`ServerConfig`] for the brief pairing window's TCP
/// listener. The first phone to connect with the matching PSK echo
/// gets bound to the offer; we accept any cert at TLS time and pin
/// it *after* the in-band `PairingComplete` arrives.
///
/// `expected_pin = None` means "accept any cert" — used only during
/// the initial pairing handshake. For steady-state inbound the
/// desktop dials out, not the other way around, so this path is
/// pairing-only.
pub fn pairing_server_config(
    desktop_cert: CertificateDer<'static>,
    desktop_key: PrivateKeyDer<'static>,
) -> Result<ServerConfig, TlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(AcceptAnyClientCert::new())
        .with_single_cert(vec![desktop_cert], desktop_key)?;
    Ok(cfg)
}

/// Trivial ClientCertVerifier used only during the pairing window:
/// accepts whatever cert the phone presents, because we don't yet
/// know its pin. The cert bytes are then checked against the
/// `PairingComplete` payload at the application layer.
#[derive(Debug)]
struct AcceptAnyClientCert {
    crypto: Arc<CryptoProvider>,
    no_dn: Vec<DistinguishedName>,
}

impl AcceptAnyClientCert {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            crypto: Arc::new(rustls::crypto::ring::default_provider()),
            no_dn: Vec::new(),
        })
    }
}

impl ClientCertVerifier for AcceptAnyClientCert {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &self.no_dn
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.crypto.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.crypto.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.crypto
            .signature_verification_algorithms
            .supported_schemes()
    }
}
