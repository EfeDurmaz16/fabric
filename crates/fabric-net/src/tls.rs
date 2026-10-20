//! Pinned TLS (ADR 0005).
//!
//! Invariant: a handshake completes only when the peer's certificate carries
//! an ed25519 key equal to the node id this side expects and the peer signed
//! the handshake with that key. The client expects the id it dialed; the
//! server expects any paired id. No CA, no name and no date take part. The
//! certificate is only the carrier of the key: the signature over the
//! handshake is checked against the expected key itself, so a copied
//! certificate without its private key fails.
//!
//! Session resumption is off on both sides. A resumed TLS 1.3 handshake
//! restores the peer's certificate from the earlier session and never asks
//! the verifier again, so a node unpaired since then would keep access.
//! Every handshake is a full one and runs the pairing check.

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

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
#[derive(Debug, Clone, thiserror::Error)]
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
    /// The peer's handshake signature was not made with the key its certificate names.
    #[error("the handshake signature does not match the certificate key")]
    BadSignature,
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

/// What the client verifier refused during one dial, kept so the dialer can report it.
///
/// quinn turns a verifier's error into text, and tokio-rustls wraps it in an
/// `io::Error`, so the verifier records its refusal here as well. A dialer
/// that finds one knows the peer at the address is the wrong node or a
/// forgery, and that another wire to the same address would not help.
#[derive(Debug, Default)]
pub(crate) struct Refusal(Mutex<Option<TlsError>>);

impl Refusal {
    fn record(&self, err: &TlsError) {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        slot.get_or_insert_with(|| err.clone());
    }

    /// Takes the recorded refusal, if the verifier made one.
    pub(crate) fn take(&self) -> Option<TlsError> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
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

    /// A client configuration that accepts only the server `expected` and records a refusal in `refusal`.
    pub(crate) fn client_config(
        &self,
        expected: NodeId,
        refusal: &Arc<Refusal>,
    ) -> Result<rustls::ClientConfig, TlsError> {
        let config = client_builder(expected, refusal)?
            .with_client_auth_cert(vec![self.cert.clone()], self.key.clone_key())
            .map_err(TlsError::Config)?;
        Ok(finish_client(config))
    }

    /// A server configuration that accepts only paired clients.
    pub(crate) fn server_config(
        &self,
        trust: Arc<dyn Trust>,
    ) -> Result<rustls::ServerConfig, TlsError> {
        let config = server_builder(trust)?
            .with_single_cert(vec![self.cert.clone()], self.key.clone_key())
            .map_err(TlsError::Config)?;
        Ok(finish_server(config))
    }
}

/// The client side up to its own certificate: TLS 1.3 and the pinned verifier.
fn client_builder(
    expected: NodeId,
    refusal: &Arc<Refusal>,
) -> Result<rustls::ConfigBuilder<rustls::ClientConfig, rustls::client::WantsClientCert>, TlsError>
{
    let provider = provider();
    let algorithms = provider.signature_verification_algorithms;
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(TlsError::Config)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServer {
            expected,
            algorithms,
            refusal: Arc::clone(refusal),
        })))
}

/// The server side up to its own certificate: TLS 1.3 and the paired-clients verifier.
fn server_builder(
    trust: Arc<dyn Trust>,
) -> Result<rustls::ConfigBuilder<rustls::ServerConfig, rustls::server::WantsServerCert>, TlsError>
{
    let provider = provider();
    let algorithms = provider.signature_verification_algorithms;
    Ok(rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(TlsError::Config)?
        .with_client_cert_verifier(Arc::new(PairedClients { trust, algorithms })))
}

fn finish_client(mut config: rustls::ClientConfig) -> rustls::ClientConfig {
    config.alpn_protocols = vec![ALPN.to_vec()];
    config.resumption = rustls::client::Resumption::disabled();
    config
}

fn finish_server(mut config: rustls::ServerConfig) -> rustls::ServerConfig {
    config.alpn_protocols = vec![ALPN.to_vec()];
    config.max_early_data_size = 0;
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    config
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
    refusal: Arc<Refusal>,
}

impl PinnedServer {
    fn check(&self, cert: &CertificateDer<'_>) -> Result<(), rustls::Error> {
        let refused = match node_id_of(cert) {
            Ok(actual) if actual == self.expected => return Ok(()),
            Ok(actual) => TlsError::PeerMismatch {
                expected: self.expected,
                actual,
            },
            Err(err) => err,
        };
        self.refusal.record(&refused);
        Err(reject(refused))
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
            .inspect_err(|_| self.refusal.record(&TlsError::BadSignature))
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

    /// A certificate and key pair installed through a resolver, which rustls
    /// does not check for consistency the way `with_single_cert` does.
    #[derive(Debug)]
    struct Forged(Arc<rustls::sign::CertifiedKey>);

    impl Forged {
        fn new(cert: &CertificateDer<'static>, key_of: &NodeIdentity) -> Arc<Self> {
            let signer = provider()
                .key_provider
                .load_private_key(PrivateKeyDer::Pkcs8(key_of.pkcs8_der()))
                .unwrap();
            Arc::new(Self(Arc::new(rustls::sign::CertifiedKey::new(
                vec![cert.clone()],
                signer,
            ))))
        }
    }

    impl rustls::client::ResolvesClientCert for Forged {
        fn resolve(
            &self,
            _root_hint_subjects: &[&[u8]],
            _sigschemes: &[SignatureScheme],
        ) -> Option<Arc<rustls::sign::CertifiedKey>> {
            Some(self.0.clone())
        }

        fn has_certs(&self) -> bool {
            true
        }
    }

    impl rustls::server::ResolvesServerCert for Forged {
        fn resolve(
            &self,
            _client_hello: rustls::server::ClientHello<'_>,
        ) -> Option<Arc<rustls::sign::CertifiedKey>> {
            Some(self.0.clone())
        }
    }

    struct Outcome {
        server: std::io::Result<()>,
        client: std::io::Result<()>,
    }

    /// Runs one handshake over an in-memory pipe and reports each side's verdict.
    ///
    /// Each side sends one byte and reads the other's, so neither side ends
    /// before the other's verdict on the handshake reached it.
    async fn handshake(
        server: Arc<rustls::ServerConfig>,
        client: Arc<rustls::ClientConfig>,
    ) -> Outcome {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio_rustls::{TlsAcceptor, TlsConnector};

        let (client_io, server_io) = tokio::io::duplex(64 << 10);
        let acceptor = TlsAcceptor::from(server);
        let accepted = tokio::spawn(async move {
            let mut tls = acceptor.accept(server_io).await?;
            tls.read_u8().await?;
            tls.write_u8(b's').await?;
            tls.flush().await
        });
        let name = ServerName::IpAddress(rustls::pki_types::IpAddr::try_from("127.0.0.1").unwrap());
        let client = async {
            let mut tls = TlsConnector::from(client).connect(name, client_io).await?;
            tls.write_u8(b'c').await?;
            tls.flush().await?;
            tls.read_u8().await.map(drop)
        }
        .await;
        Outcome {
            server: accepted.await.unwrap(),
            client,
        }
    }

    /// The local verifier found a handshake signature that the certificate's key did not make.
    fn is_bad_signature(result: &std::io::Result<()>) -> bool {
        matches!(
            result
                .as_ref()
                .err()
                .and_then(|err| err.get_ref())
                .and_then(|err| err.downcast_ref::<rustls::Error>()),
            Some(rustls::Error::InvalidCertificate(
                CertificateError::BadSignature
            ))
        )
    }

    /// A certificate is public, so each verifier checks the handshake
    /// signature against the key the certificate names. A side that holds a
    /// node's certificate with another private key fails inside the
    /// handshake, as a dialer and as a server. rustls refuses a mismatched
    /// pair when a config is built, which would make this test pass for the
    /// wrong reason; the forged side installs its pair through a resolver,
    /// so the handshake runs and the verifier is what refuses.
    #[tokio::test]
    async fn a_certificate_without_its_key_fails_the_handshake_on_either_side() {
        let server = NodeIdentity::generate().unwrap();
        let client = NodeIdentity::generate().unwrap();
        let thief = NodeIdentity::generate().unwrap();
        let server_creds = Credentials::new(&server).unwrap();
        let client_creds = Credentials::new(&client).unwrap();
        let trust: Arc<dyn Trust> = Arc::new(Paired(vec![client.node_id()]));

        let honest = handshake(
            Arc::new(server_creds.server_config(trust.clone()).unwrap()),
            Arc::new(
                client_creds
                    .client_config(server.node_id(), &Arc::default())
                    .unwrap(),
            ),
        )
        .await;
        assert!(honest.server.is_ok() && honest.client.is_ok());

        let forged_dialer = finish_client(
            client_builder(server.node_id(), &Arc::default())
                .unwrap()
                .with_client_cert_resolver(Forged::new(&client_creds.cert, &thief)),
        );
        let outcome = handshake(
            Arc::new(server_creds.server_config(trust.clone()).unwrap()),
            Arc::new(forged_dialer),
        )
        .await;
        assert!(
            is_bad_signature(&outcome.server),
            "the server must refuse the forged dialer: {:?}",
            outcome.server
        );

        let forged_server = finish_server(
            server_builder(trust)
                .unwrap()
                .with_cert_resolver(Forged::new(&server_creds.cert, &thief)),
        );
        let outcome = handshake(
            Arc::new(forged_server),
            Arc::new(
                client_creds
                    .client_config(server.node_id(), &Arc::default())
                    .unwrap(),
            ),
        )
        .await;
        assert!(
            is_bad_signature(&outcome.client),
            "the dialer must refuse the forged server: {:?}",
            outcome.client
        );
    }

    /// A trust set the test changes between handshakes, as an unpair does.
    #[derive(Debug, Default)]
    struct Switch(std::sync::Mutex<Vec<NodeId>>);

    impl Trust for Switch {
        fn is_paired(&self, node: NodeId) -> bool {
            self.0.lock().unwrap().contains(&node)
        }
    }

    /// rustls resumes TLS 1.3 sessions by default and a resumed handshake
    /// skips the client verifier. A node unpaired after its first connection
    /// must be refused on its next one, even when it dials with the same
    /// client configuration that holds the earlier session.
    #[tokio::test]
    async fn an_unpaired_node_is_refused_on_reconnect_with_the_same_config() {
        let server = NodeIdentity::generate().unwrap();
        let client = NodeIdentity::generate().unwrap();
        let trust = Arc::new(Switch::default());
        trust.0.lock().unwrap().push(client.node_id());
        let server_config = Arc::new(
            Credentials::new(&server)
                .unwrap()
                .server_config(trust.clone())
                .unwrap(),
        );
        let client_config = Arc::new(
            Credentials::new(&client)
                .unwrap()
                .client_config(server.node_id(), &Arc::default())
                .unwrap(),
        );

        let first = handshake(server_config.clone(), client_config.clone()).await;
        assert!(first.server.is_ok() && first.client.is_ok());

        trust.0.lock().unwrap().clear();
        let second = handshake(server_config, client_config).await;
        assert!(
            second.server.is_err(),
            "an unpaired node must not resume its way back in"
        );
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
