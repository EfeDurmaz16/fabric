//! Pinned TLS (ADR 0005).
//!
//! Invariant: a handshake completes only when the peer's certificate carries
//! an ed25519 key equal to the node id this side expects and the peer signed
//! the handshake with that key. The client expects the id it dialed; the
//! server expects any paired id. No CA, no name and no date take part. The
//! certificate is only the carrier of the key: the signature over the
//! handshake is checked against the expected key itself, so a copied
//! certificate without its private key fails.

use std::fmt;
use std::sync::{Arc, OnceLock};

use fabric_core::id::NodeId;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{
    CryptoProvider, WebPkiSupportedAlgorithms, verify_tls13_signature_with_raw_key,
};
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, ServerName, SubjectPublicKeyInfoDer, UnixTime,
};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, OtherError, SignatureScheme,
};

use crate::identity::NodeIdentity;

/// The DER prefix of an ed25519 `SubjectPublicKeyInfo`; the 32 key bytes follow it.
pub const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// The ALPN protocol both sides must offer.
pub const ALPN: &[u8] = b"fabric/1";

/// The server name a QUIC client dials; the verifier never reads it, but TLS needs one.
pub(crate) const SERVER_NAME: &str = "fabric";

/// Why a certificate or a TLS configuration was refused.
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    /// The certificate does not parse as X.509.
    #[error("the certificate does not parse")]
    BadCertificate,
    /// The certificate's key is not ed25519, so it names no node.
    #[error("the certificate key is not ed25519")]
    NotEd25519,
    /// The peer is a node, but not the one this side dialed.
    #[error("the peer is {actual}, not {expected}")]
    PeerMismatch {
        /// The id this side expected.
        expected: NodeId,
        /// The id the certificate carries.
        actual: NodeId,
    },
    /// The peer is a node this side has not paired with.
    #[error("{0} is not paired")]
    Unpaired(NodeId),
    /// The peer sent no certificate.
    #[error("the peer sent no certificate")]
    NoCertificate,
    /// rustls refused the configuration.
    #[error("tls configuration: {0}")]
    Config(#[source] rustls::Error),
    /// The self-signed certificate could not be made.
    #[error("certificate generation: {0}")]
    Certificate(#[source] rcgen::Error),
}

/// The accept-side trust decision: is this node one we paired with?
pub trait Trust: Send + Sync + fmt::Debug {
    /// Returns true when `node` may connect to this node.
    fn is_paired(&self, node: NodeId) -> bool;
}

/// The one crypto provider of the crate, ring, shared by TLS and key generation.
pub(crate) fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

/// Reads the node id out of a certificate: its ed25519 public key.
pub fn node_id_of(cert: &CertificateDer<'_>) -> Result<NodeId, TlsError> {
    let cert = webpki::EndEntityCert::try_from(cert).map_err(|_| TlsError::BadCertificate)?;
    let spki = cert.subject_public_key_info();
    let (prefix, key) = spki
        .as_ref()
        .split_at_checked(ED25519_SPKI_PREFIX.len())
        .ok_or(TlsError::NotEd25519)?;
    if prefix != ED25519_SPKI_PREFIX {
        return Err(TlsError::NotEd25519);
    }
    let key: [u8; 32] = key.try_into().map_err(|_| TlsError::NotEd25519)?;
    Ok(NodeId::from_bytes(key))
}

/// Reads the node id of the first certificate in a peer's chain.
pub(crate) fn peer_of(certs: Option<&[CertificateDer<'_>]>) -> Result<NodeId, TlsError> {
    let first = certs
        .and_then(|c| c.first())
        .ok_or(TlsError::NoCertificate)?;
    node_id_of(first)
}

fn spki_of(node: NodeId) -> SubjectPublicKeyInfoDer<'static> {
    let mut der = Vec::with_capacity(ED25519_SPKI_PREFIX.len() + 32);
    der.extend_from_slice(&ED25519_SPKI_PREFIX);
    der.extend_from_slice(&node.to_bytes());
    SubjectPublicKeyInfoDer::from(der)
}

/// This node's certificate and private key, made once per identity.
#[derive(Debug)]
pub(crate) struct Credentials {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

impl Credentials {
    /// Makes a self-signed certificate whose key is the node key.
    pub(crate) fn new(identity: &NodeIdentity) -> Result<Self, TlsError> {
        let mut params =
            rcgen::CertificateParams::new(Vec::<String>::new()).map_err(TlsError::Certificate)?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, identity.node_id().to_string());
        let cert = params
            .self_signed(identity.key_pair())
            .map_err(TlsError::Certificate)?;
        Ok(Self {
            cert: cert.der().clone(),
            key: PrivateKeyDer::Pkcs8(identity.pkcs8_der()),
        })
    }

    /// A client configuration that accepts only the server `expected`.
    pub(crate) fn client_config(&self, expected: NodeId) -> Result<rustls::ClientConfig, TlsError> {
        let provider = provider();
        let algorithms = provider.signature_verification_algorithms;
        let mut config = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(TlsError::Config)?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedServer {
                expected,
                algorithms,
            }))
            .with_client_auth_cert(vec![self.cert.clone()], self.key.clone_key())
            .map_err(TlsError::Config)?;
        config.alpn_protocols = vec![ALPN.to_vec()];
        Ok(config)
    }

    /// A server configuration that accepts only paired clients.
    pub(crate) fn server_config(
        &self,
        trust: Arc<dyn Trust>,
    ) -> Result<rustls::ServerConfig, TlsError> {
        let provider = provider();
        let algorithms = provider.signature_verification_algorithms;
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(TlsError::Config)?
            .with_client_cert_verifier(Arc::new(PairedClients { trust, algorithms }))
            .with_single_cert(vec![self.cert.clone()], self.key.clone_key())
            .map_err(TlsError::Config)?;
        config.alpn_protocols = vec![ALPN.to_vec()];
        config.max_early_data_size = 0;
        Ok(config)
    }
}

fn reject(err: TlsError) -> rustls::Error {
    tracing::debug!(error = %err, "refusing a peer certificate");
    rustls::Error::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(err))))
}

fn no_tls12() -> rustls::Error {
    rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered)
}

/// Accepts one server: the node id the client dialed.
#[derive(Debug)]
struct PinnedServer {
    expected: NodeId,
    algorithms: WebPkiSupportedAlgorithms,
}

impl PinnedServer {
    fn check(&self, cert: &CertificateDer<'_>) -> Result<(), rustls::Error> {
        let actual = node_id_of(cert).map_err(reject)?;
        if actual != self.expected {
            return Err(reject(TlsError::PeerMismatch {
                expected: self.expected,
                actual,
            }));
        }
        Ok(())
    }
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.check(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(no_tls12())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.check(cert)?;
        verify_tls13_signature_with_raw_key(message, &spki_of(self.expected), dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

/// Accepts any client whose node id is paired.
#[derive(Debug)]
struct PairedClients {
    trust: Arc<dyn Trust>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl PairedClients {
    fn check(&self, cert: &CertificateDer<'_>) -> Result<NodeId, rustls::Error> {
        let node = node_id_of(cert).map_err(reject)?;
        if !self.trust.is_paired(node) {
            return Err(reject(TlsError::Unpaired(node)));
        }
        Ok(node)
    }
}

impl ClientCertVerifier for PairedClients {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.check(end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(no_tls12())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let node = self.check(cert)?;
        verify_tls13_signature_with_raw_key(message, &spki_of(node), dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_certificate_carries_the_node_id() {
        let identity = NodeIdentity::generate().unwrap();
        let creds = Credentials::new(&identity).unwrap();
        assert_eq!(node_id_of(&creds.cert).unwrap(), identity.node_id());
        assert_eq!(
            spki_of(identity.node_id()).as_ref(),
            webpki::EndEntityCert::try_from(&creds.cert)
                .unwrap()
                .subject_public_key_info()
                .as_ref()
        );
    }

    #[derive(Debug)]
    struct Paired(Vec<NodeId>);

    impl Trust for Paired {
        fn is_paired(&self, node: NodeId) -> bool {
            self.0.contains(&node)
        }
    }

    /// The server pins the key, not the certificate: a dialer that holds a
    /// paired node's certificate but another private key cannot complete
    /// the handshake, because the signature over the handshake is checked
    /// against the key the certificate names.
    #[tokio::test]
    async fn a_paired_certificate_without_its_key_fails_the_handshake() {
        use tokio::io::AsyncWriteExt;
        use tokio_rustls::{TlsAcceptor, TlsConnector};

        let server = NodeIdentity::generate().unwrap();
        let honest = NodeIdentity::generate().unwrap();
        let thief = NodeIdentity::generate().unwrap();
        let server_creds = Credentials::new(&server).unwrap();
        let honest_creds = Credentials::new(&honest).unwrap();
        let forged = Credentials {
            cert: honest_creds.cert.clone(),
            key: Credentials::new(&thief).unwrap().key,
        };
        let trust: Arc<dyn Trust> = Arc::new(Paired(vec![honest.node_id()]));
        let acceptor = TlsAcceptor::from(Arc::new(server_creds.server_config(trust).unwrap()));
        let name = ServerName::IpAddress(rustls::pki_types::IpAddr::try_from("127.0.0.1").unwrap());

        let handshake = |creds: &Credentials| {
            let config = creds.client_config(server.node_id());
            let acceptor = acceptor.clone();
            let name = name.clone();
            async move {
                let config = config?;
                let (client_io, server_io) = tokio::io::duplex(64 << 10);
                let accepted = tokio::spawn(async move { acceptor.accept(server_io).await });
                let connector = TlsConnector::from(Arc::new(config));
                let mut client = connector
                    .connect(name, client_io)
                    .await
                    .map_err(|e| TlsError::Config(rustls::Error::General(e.to_string())))?;
                // The client believes the handshake is done once it sent its
                // Finished; the server's verdict shows up on the first read.
                client.write_all(b"x").await.ok();
                let _ = client.shutdown().await;
                accepted
                    .await
                    .map_err(|e| TlsError::Config(rustls::Error::General(e.to_string())))?
                    .map(|_| ())
                    .map_err(|e| TlsError::Config(rustls::Error::General(e.to_string())))
            }
        };

        assert!(handshake(&forged).await.is_err());
        handshake(&honest_creds).await.unwrap();
    }

    #[test]
    fn a_certificate_with_another_key_type_names_no_node() {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let cert = rcgen::CertificateParams::new(Vec::<String>::new())
            .unwrap()
            .self_signed(&key)
            .unwrap();
        assert!(matches!(node_id_of(cert.der()), Err(TlsError::NotEd25519)));
        assert!(matches!(
            node_id_of(&CertificateDer::from(vec![0x30, 0x00])),
            Err(TlsError::BadCertificate)
        ));
    }
}
