use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::Context;
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use teamview_protocol::control::{
    ControlError, ServerControl, ServerEnvelope, decode_client_envelope, encode_server_envelope,
};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::{control::ControlState, session::Session};

const CONTROL_STREAM_READ_LIMIT: usize = 64 * 1024;
const CONTROL_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct ControlRuntime {
    state: Arc<Mutex<ControlState>>,
    next_session_id: Arc<AtomicU64>,
}

impl ControlRuntime {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ControlState::new())),
            next_session_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub async fn serve_endpoint(self, endpoint: Endpoint) {
        while let Some(incoming) = endpoint.accept().await {
            let runtime = self.clone();
            tokio::spawn(async move {
                match incoming.await {
                    Ok(connection) => runtime.serve_connection(connection).await,
                    Err(error) => warn!(%error, "failed to accept QUIC connection"),
                }
            });
        }
    }

    pub(crate) async fn serve_connection(&self, connection: Connection) {
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let mut session = Session::anonymous(session_id);
        info!(
            session_id,
            remote = %connection.remote_address(),
            "accepted relay control connection"
        );

        loop {
            match connection.accept_bi().await {
                Ok((send, recv)) => {
                    if let Err(error) = self.handle_control_stream(&mut session, send, recv).await {
                        warn!(session_id, %error, "control stream failed");
                    }
                }
                Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
                Err(quinn::ConnectionError::LocallyClosed) => break,
                Err(quinn::ConnectionError::TimedOut) => break,
                Err(error) => {
                    debug!(session_id, %error, "control connection closed");
                    break;
                }
            }
        }
    }

    async fn handle_control_stream(
        &self,
        session: &mut Session,
        mut send: SendStream,
        mut recv: RecvStream,
    ) -> anyhow::Result<()> {
        let bytes = tokio::time::timeout(
            CONTROL_STREAM_IDLE_TIMEOUT,
            recv.read_to_end(CONTROL_STREAM_READ_LIMIT),
        )
        .await
        .context("timed out waiting for control request")?
        .context("failed to read control request")?;

        let response = self.handle_control_bytes(session, &bytes).await;
        let response_bytes =
            encode_server_envelope(&response).context("failed to encode response")?;
        send.write_all(&response_bytes)
            .await
            .context("failed to write control response")?;
        send.finish().context("failed to finish control response")?;
        Ok(())
    }

    async fn handle_control_bytes(&self, session: &mut Session, bytes: &[u8]) -> ServerEnvelope {
        match decode_client_envelope(bytes) {
            Ok(envelope) => self
                .state
                .lock()
                .await
                .handle_client_envelope(session, envelope),
            Err(error) => {
                error!(%error, "failed to decode control request");
                ServerEnvelope::new(
                    0,
                    ServerControl::Error(ControlError::new(
                        "bad_control_message",
                        error.to_string(),
                    )),
                )
            }
        }
    }
}

impl Default for ControlRuntime {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn serve_control_endpoint(endpoint: Endpoint) {
    ControlRuntime::new().serve_endpoint(endpoint).await;
}

#[cfg(test)]
mod tests {
    use quinn::ClientConfig;
    use teamview_protocol::{
        PROTOCOL_VERSION,
        control::{
            ClientControl, ClientEnvelope, Hello, ServerControl, decode_server_envelope,
            encode_client_envelope,
        },
    };

    use crate::transport::build_server_endpoint;

    use super::*;

    #[tokio::test]
    async fn control_runtime_accepts_hello_bytes() {
        let runtime = ControlRuntime::new();
        let mut session = Session::anonymous(1);
        let request = ClientEnvelope::new(
            7,
            ClientControl::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                client_name: "desktop-client-test".to_owned(),
            }),
        );
        let bytes = encode_client_envelope(&request).unwrap();

        let response = runtime.handle_control_bytes(&mut session, &bytes).await;

        assert_eq!(response.request_id, 7);
        assert!(matches!(response.message, ServerControl::HelloAccepted(_)));
        assert!(session.user_id.is_some());
    }

    #[tokio::test]
    async fn control_runtime_accepts_hello_over_quic_stream() {
        let server = build_server_endpoint("127.0.0.1:0").unwrap();
        let server_addr = server.local_addr().unwrap();
        let runtime = ControlRuntime::new();
        let server_task = tokio::spawn(async move {
            let incoming = server.accept().await.expect("incoming connection");
            let connection = incoming.await.expect("accepted connection");
            runtime.serve_connection(connection).await;
        });

        let mut client = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        client.set_default_client_config(build_insecure_local_client_config());
        let connection = client
            .connect(server_addr, "localhost")
            .unwrap()
            .await
            .expect("client connects");
        let (mut send, mut recv) = connection.open_bi().await.expect("control stream opens");
        let request = ClientEnvelope::new(
            42,
            ClientControl::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                client_name: "integration-client".to_owned(),
            }),
        );
        send.write_all(&encode_client_envelope(&request).unwrap())
            .await
            .expect("request writes");
        send.finish().expect("request finishes");

        let response_bytes = recv
            .read_to_end(CONTROL_STREAM_READ_LIMIT)
            .await
            .expect("response reads");
        let response = decode_server_envelope(&response_bytes).unwrap();

        assert_eq!(response.request_id, 42);
        assert!(matches!(response.message, ServerControl::HelloAccepted(_)));
        connection.close(0_u32.into(), b"done");
        server_task.await.unwrap();
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
}
