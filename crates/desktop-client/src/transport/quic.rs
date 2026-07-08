use std::{net::SocketAddr, sync::Arc, time::Duration};

use super::RelayEndpoint;
use anyhow::Context;
use quinn::{ClientConfig, Endpoint};
use teamview_protocol::control::{
    ClientEnvelope, ServerEnvelope, decode_server_envelope, encode_client_envelope,
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

pub async fn send_control_request(
    endpoint: &Endpoint,
    relay_addr: &str,
    request: &ClientEnvelope,
) -> anyhow::Result<ServerEnvelope> {
    let relay_addr: SocketAddr = relay_addr
        .parse()
        .with_context(|| format!("invalid relay address {relay_addr}"))?;
    let connection = endpoint
        .connect(relay_addr, "localhost")
        .context("failed to start QUIC connection")?
        .await
        .context("failed to connect to relay")?;
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
}
