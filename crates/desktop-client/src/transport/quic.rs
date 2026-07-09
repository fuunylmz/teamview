use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use super::RelayEndpoint;
use anyhow::Context;
use quinn::{ClientConfig, Connection, Endpoint};
use teamview_protocol::{
    control::{
        ClientControl, ClientEnvelope, RequestId, ServerEnvelope, decode_server_envelope,
        encode_client_envelope,
    },
    packet::MediaPacket,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuicClientConfig {
    pub relay: RelayEndpoint,
    pub max_datagram_payload: usize,
}

impl QuicClientConfig {
    pub fn new(relay: RelayEndpoint) -> Self {
        Self {
            relay,
            max_datagram_payload: teamview_protocol::packet::DEFAULT_DATAGRAM_PAYLOAD_TARGET,
        }
    }
}

const CONTROL_STREAM_READ_LIMIT: usize = 64 * 1024;
const CONTROL_STREAM_TIMEOUT: Duration = Duration::from_secs(5);

pub fn build_client_endpoint(bind_addr: &str) -> anyhow::Result<Endpoint> {
    let addr: SocketAddr = bind_addr
        .parse()
        .with_context(|| format!("invalid bind address {bind_addr}"))?;
    let mut endpoint = Endpoint::client(addr).context("failed to bind QUIC client endpoint")?;
    endpoint.set_default_client_config(build_insecure_local_client_config());
    Ok(endpoint)
}

pub async fn connect_control_client(
    endpoint: &Endpoint,
    relay_addr: &str,
) -> anyhow::Result<ControlClient> {
    let relay_addr: SocketAddr = relay_addr
        .parse()
        .with_context(|| format!("invalid relay address {relay_addr}"))?;
    let connection = endpoint
        .connect(relay_addr, "localhost")
        .context("failed to start QUIC connection")?
        .await
        .context("failed to connect to relay")?;
    Ok(ControlClient::new(connection))
}

#[derive(Debug, Clone)]
pub struct ControlClient {
    connection: Connection,
    request_ids: RequestIdAllocator,
}

impl ControlClient {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            request_ids: RequestIdAllocator::new(),
        }
    }

    pub async fn send(&self, message: ClientControl) -> anyhow::Result<ServerEnvelope> {
        let request_id = self.request_ids.allocate();
        self.send_envelope(&ClientEnvelope::new(request_id, message))
            .await
    }

    pub async fn send_envelope(&self, request: &ClientEnvelope) -> anyhow::Result<ServerEnvelope> {
        send_control_request_on_connection(&self.connection, request).await
    }

    pub fn send_media_packet(&self, packet: &MediaPacket) -> anyhow::Result<()> {
        let bytes = packet.encode().context("failed to encode media packet")?;
        self.connection
            .send_datagram(bytes)
            .context("failed to send media datagram")
    }

    pub async fn recv_media_packet(&self) -> anyhow::Result<MediaPacket> {
        let bytes = self
            .connection
            .read_datagram()
            .await
            .context("failed to read media datagram")?;
        MediaPacket::decode(&bytes).context("failed to decode media packet")
    }
}

#[derive(Debug, Clone)]
struct RequestIdAllocator {
    next_request_id: Arc<AtomicU64>,
}

impl RequestIdAllocator {
    fn new() -> Self {
        Self {
            next_request_id: Arc::new(AtomicU64::new(1)),
        }
    }

    fn allocate(&self) -> RequestId {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }
}

pub async fn send_control_request(
    endpoint: &Endpoint,
    relay_addr: &str,
    request: &ClientEnvelope,
) -> anyhow::Result<ServerEnvelope> {
    let client = connect_control_client(endpoint, relay_addr).await?;
    client.send_envelope(request).await
}

async fn send_control_request_on_connection(
    connection: &Connection,
    request: &ClientEnvelope,
) -> anyhow::Result<ServerEnvelope> {
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .context("failed to open control stream")?;
    let request_bytes =
        encode_client_envelope(request).context("failed to encode control request")?;
    send.write_all(&request_bytes)
        .await
        .context("failed to write control request")?;
    send.finish().context("failed to finish control request")?;

    let response_bytes = tokio::time::timeout(
        CONTROL_STREAM_TIMEOUT,
        recv.read_to_end(CONTROL_STREAM_READ_LIMIT),
    )
    .await
    .context("timed out waiting for control response")?
    .context("failed to read control response")?;
    decode_server_envelope(&response_bytes).context("failed to decode control response")
}

fn build_insecure_local_client_config() -> ClientConfig {
    let mut crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"teamview-stage1".to_vec()];
    ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
            .expect("valid local QUIC client config"),
    ))
}

#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_bind_addr() {
        assert!(build_client_endpoint("not-an-addr").is_err());
    }

    #[test]
    fn cloned_request_id_allocators_share_sequence() {
        let allocator = RequestIdAllocator::new();
        let cloned = allocator.clone();

        assert_eq!(allocator.allocate(), 1);
        assert_eq!(cloned.allocate(), 2);
        assert_eq!(allocator.allocate(), 3);
    }
}
