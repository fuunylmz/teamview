use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::Context;
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use teamview_protocol::{
    control::{
        ControlError, ServerControl, ServerEnvelope, decode_client_envelope, encode_server_envelope,
    },
    packet::MediaPacket,
};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::{
    config::ServerConfig,
    control::ControlState,
    media::MediaRelay,
    metrics::{micros_delta_to_millis, unix_time_micros},
    session::Session,
};

const CONTROL_STREAM_READ_LIMIT: usize = 64 * 1024;
const CONTROL_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct ControlRuntime {
    state: Arc<Mutex<ControlState>>,
    media: Arc<Mutex<MediaRelay>>,
    next_session_id: Arc<AtomicU64>,
}

impl ControlRuntime {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ControlState::new())),
            media: Arc::new(Mutex::new(MediaRelay::new())),
            next_session_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn from_config(config: ServerConfig) -> Self {
        let state = match config.access_token {
            Some(access_token) => ControlState::with_access_token(access_token),
            None => ControlState::new(),
        };
        Self {
            state: Arc::new(Mutex::new(state)),
            media: Arc::new(Mutex::new(MediaRelay::with_viewer_queue_budget_ms(
                config.viewer_queue_budget_ms,
            ))),
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
        let session = Arc::new(Mutex::new(Session::anonymous(session_id)));
        info!(
            session_id,
            remote = %connection.remote_address(),
            "accepted relay control connection"
        );

        loop {
            tokio::select! {
                stream = connection.accept_bi() => {
                    match stream {
                        Ok((send, recv)) => {
                            let runtime = self.clone();
                            let connection = connection.clone();
                            let session = session.clone();
                            tokio::spawn(async move {
                                if let Err(error) = runtime.handle_control_stream(&connection, session, send, recv).await {
                                    warn!(session_id, %error, "control stream failed");
                                }
                            });
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
                datagram = connection.read_datagram() => {
                    match datagram {
                        Ok(bytes) => {
                            let received_at_micros = unix_time_micros();
                            self.handle_media_datagram(session.clone(), &bytes, received_at_micros).await;
                        }
                        Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
                        Err(quinn::ConnectionError::LocallyClosed) => break,
                        Err(quinn::ConnectionError::TimedOut) => break,
                        Err(error) => {
                            debug!(session_id, %error, "media datagram loop closed");
                            break;
                        }
                    }
                }
            }
        }

        let user_id = session.lock().await.user_id;
        if let Some(user_id) = user_id {
            self.media.lock().await.unregister(user_id);
            self.state.lock().await.disconnect_user(user_id);
        }
    }

    async fn handle_control_stream(
        &self,
        connection: &Connection,
        session: Arc<Mutex<Session>>,
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

        let response = {
            let mut session = session.lock().await;
            self.handle_control_bytes(&mut session, &bytes).await
        };
        if matches!(response.message, ServerControl::HelloAccepted(_)) {
            let user_id = session.lock().await.user_id;
            if let Some(user_id) = user_id {
                self.media
                    .lock()
                    .await
                    .register(user_id, connection.clone());
            }
        }
        let response_bytes =
            encode_server_envelope(&response).context("failed to encode response")?;
        send.write_all(&response_bytes)
            .await
            .context("failed to write control response")?;
        send.finish().context("failed to finish control response")?;
        Ok(())
    }

    async fn handle_media_datagram(
        &self,
        session: Arc<Mutex<Session>>,
        bytes: &[u8],
        received_at_micros: u64,
    ) {
        let session = session.lock().await;
        let Some(user_id) = session.user_id else {
            debug!(
                session_id = session.id,
                "dropping media datagram before Hello"
            );
            return;
        };
        if !session.access_granted {
            debug!(
                session_id = session.id,
                user_id, "dropping media datagram before authentication"
            );
            return;
        }
        let session_id = session.id;
        drop(session);
        let packet = match MediaPacket::decode(bytes) {
            Ok(packet) => packet,
            Err(error) => {
                debug!(session_id, %error, "dropping malformed media datagram");
                return;
            }
        };
        let mut state = self.state.lock().await;
        let media = self.media.lock().await;
        let summary = media.forward_media_packet(&state, user_id, &packet);
        let server_route_ms = micros_delta_to_millis(received_at_micros, unix_time_micros());
        state.record_media_forward_summary(
            &packet,
            summary,
            bytes.len(),
            received_at_micros,
            server_route_ms,
        );
        debug!(
            session_id,
            user_id,
            stream_id = summary.stream_id,
            queued = summary.queued,
            dropped = summary.dropped,
            "forwarded media datagram"
        );
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

pub async fn serve_control_endpoint(endpoint: Endpoint, config: ServerConfig) {
    ControlRuntime::from_config(config)
        .serve_endpoint(endpoint)
        .await;
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use quinn::ClientConfig;
    use teamview_protocol::{
        PROTOCOL_VERSION,
        codec::CodecId,
        control::{
            Authenticate, ClientControl, ClientEnvelope, CreateRoom, Hello, JoinRoom, MediaKind,
            PollStreamMetrics, PublishStream, ServerControl, SubscribeStream,
            decode_server_envelope, encode_client_envelope,
        },
        packet::{MediaPacket, MediaPacketHeader, PacketFlags, PacketType},
    };

    use crate::{config::ServerConfig, transport::build_server_endpoint};

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

    #[tokio::test]
    async fn control_runtime_handles_room_flow_over_multiple_quic_streams() {
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

        let hello = send_control_request(
            &connection,
            ClientEnvelope::new(
                1,
                ClientControl::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "integration-client".to_owned(),
                }),
            ),
        )
        .await;
        assert_eq!(hello.request_id, 1);
        assert!(matches!(hello.message, ServerControl::HelloAccepted(_)));

        let created = send_control_request(
            &connection,
            ClientEnvelope::new(
                2,
                ClientControl::CreateRoom(CreateRoom {
                    name: "stage1".to_owned(),
                }),
            ),
        )
        .await;
        let room_id = match created.message {
            ServerControl::RoomCreated(room) => {
                assert_eq!(created.request_id, 2);
                room.room_id
            }
            other => panic!("unexpected create room response: {other:?}"),
        };

        let joined = send_control_request(
            &connection,
            ClientEnvelope::new(3, ClientControl::JoinRoom(JoinRoom { room_id })),
        )
        .await;
        assert_eq!(joined.request_id, 3);
        assert!(matches!(joined.message, ServerControl::RoomJoined(_)));

        let published = send_control_request(
            &connection,
            ClientEnvelope::new(
                4,
                ClientControl::PublishStream(PublishStream {
                    room_id,
                    stream_id: 9,
                    codec: CodecId::H264,
                    media_kind: MediaKind::Screen,
                }),
            ),
        )
        .await;
        assert_eq!(published.request_id, 4);
        assert!(matches!(
            published.message,
            ServerControl::StreamPublished(_)
        ));

        let subscribed = send_control_request(
            &connection,
            ClientEnvelope::new(
                5,
                ClientControl::SubscribeStream(SubscribeStream {
                    room_id,
                    stream_id: 9,
                }),
            ),
        )
        .await;
        assert_eq!(subscribed.request_id, 5);
        assert!(matches!(
            subscribed.message,
            ServerControl::StreamSubscribed(_)
        ));

        connection.close(0_u32.into(), b"done");
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn control_runtime_requires_access_token_when_configured() {
        let server = build_server_endpoint("127.0.0.1:0").unwrap();
        let server_addr = server.local_addr().unwrap();
        let runtime = ControlRuntime::from_config(
            ServerConfig::new("127.0.0.1:0".to_owned())
                .with_access_token(Some("secret".to_owned())),
        );
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

        send_control_request(
            &connection,
            ClientEnvelope::new(
                1,
                ClientControl::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "integration-client".to_owned(),
                }),
            ),
        )
        .await;

        let blocked = send_control_request(
            &connection,
            ClientEnvelope::new(
                2,
                ClientControl::CreateRoom(CreateRoom {
                    name: "stage1".to_owned(),
                }),
            ),
        )
        .await;
        match blocked.message {
            ServerControl::Error(error) => assert_eq!(error.code, "not_authenticated"),
            other => panic!("unexpected blocked response: {other:?}"),
        }

        let invalid = send_control_request(
            &connection,
            ClientEnvelope::new(
                3,
                ClientControl::Authenticate(Authenticate {
                    token: "wrong".to_owned(),
                }),
            ),
        )
        .await;
        match invalid.message {
            ServerControl::Error(error) => assert_eq!(error.code, "invalid_token"),
            other => panic!("unexpected invalid token response: {other:?}"),
        }

        let authenticated = send_control_request(
            &connection,
            ClientEnvelope::new(
                4,
                ClientControl::Authenticate(Authenticate {
                    token: "secret".to_owned(),
                }),
            ),
        )
        .await;
        assert!(matches!(
            authenticated.message,
            ServerControl::Authenticated(_)
        ));

        let created = send_control_request(
            &connection,
            ClientEnvelope::new(
                5,
                ClientControl::CreateRoom(CreateRoom {
                    name: "stage1".to_owned(),
                }),
            ),
        )
        .await;
        assert!(matches!(created.message, ServerControl::RoomCreated(_)));

        connection.close(0_u32.into(), b"done");
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn control_runtime_forwards_media_datagram_to_subscribed_viewer() {
        let server = build_server_endpoint("127.0.0.1:0").unwrap();
        let server_addr = server.local_addr().unwrap();
        let runtime = ControlRuntime::new();
        let server_task = tokio::spawn(async move {
            while let Some(incoming) = server.accept().await {
                let runtime = runtime.clone();
                tokio::spawn(async move {
                    let connection = incoming.await.expect("accepted connection");
                    runtime.serve_connection(connection).await;
                });
            }
        });

        let mut publisher_endpoint = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        publisher_endpoint.set_default_client_config(build_insecure_local_client_config());
        let publisher = publisher_endpoint
            .connect(server_addr, "localhost")
            .unwrap()
            .await
            .expect("publisher connects");

        let mut viewer_endpoint = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        viewer_endpoint.set_default_client_config(build_insecure_local_client_config());
        let viewer = viewer_endpoint
            .connect(server_addr, "localhost")
            .unwrap()
            .await
            .expect("viewer connects");

        send_control_request(
            &publisher,
            ClientEnvelope::new(
                1,
                ClientControl::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "publisher".to_owned(),
                }),
            ),
        )
        .await;
        send_control_request(
            &viewer,
            ClientEnvelope::new(
                1,
                ClientControl::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "viewer".to_owned(),
                }),
            ),
        )
        .await;
        let created = send_control_request(
            &publisher,
            ClientEnvelope::new(
                2,
                ClientControl::CreateRoom(CreateRoom {
                    name: "stage1".to_owned(),
                }),
            ),
        )
        .await;
        let room_id = match created.message {
            ServerControl::RoomCreated(room) => room.room_id,
            other => panic!("unexpected create room response: {other:?}"),
        };
        send_control_request(
            &publisher,
            ClientEnvelope::new(3, ClientControl::JoinRoom(JoinRoom { room_id })),
        )
        .await;
        send_control_request(
            &viewer,
            ClientEnvelope::new(2, ClientControl::JoinRoom(JoinRoom { room_id })),
        )
        .await;
        send_control_request(
            &publisher,
            ClientEnvelope::new(
                4,
                ClientControl::PublishStream(PublishStream {
                    room_id,
                    stream_id: 9,
                    codec: CodecId::H264,
                    media_kind: MediaKind::Screen,
                }),
            ),
        )
        .await;
        send_control_request(
            &viewer,
            ClientEnvelope::new(
                3,
                ClientControl::SubscribeStream(SubscribeStream {
                    room_id,
                    stream_id: 9,
                }),
            ),
        )
        .await;

        let packet = synthetic_media_packet(9);
        publisher
            .send_datagram(packet.encode().unwrap())
            .expect("publisher sends datagram");
        let forwarded = tokio::time::timeout(Duration::from_secs(1), viewer.read_datagram())
            .await
            .expect("viewer receives forwarded datagram")
            .expect("datagram read succeeds");

        assert_eq!(MediaPacket::decode(&forwarded).unwrap(), packet);

        let metrics = send_control_request(
            &publisher,
            ClientEnvelope::new(
                5,
                ClientControl::PollStreamMetrics(PollStreamMetrics {
                    room_id,
                    stream_id: 9,
                }),
            ),
        )
        .await;
        match metrics.message {
            ServerControl::StreamMetrics(metrics) => {
                assert_eq!(metrics.room_id, room_id);
                assert_eq!(metrics.stream_id, 9);
                assert_eq!(metrics.ingress_packets, 1);
                assert_eq!(metrics.egress_queued_packets, 1);
                assert_eq!(metrics.egress_dropped_packets, 0);
                assert_eq!(metrics.subscriber_count, 1);
                assert!(metrics.ingress_bytes > 0);
                assert!(metrics.last_ingress_time_micros > 0);
                assert!(metrics.server_route_ms_p95 >= metrics.server_route_ms_p50);
            }
            other => panic!("unexpected metrics response: {other:?}"),
        }

        publisher.close(0_u32.into(), b"done");
        viewer.close(0_u32.into(), b"done");
        server_task.abort();
    }

    fn synthetic_media_packet(stream_id: u32) -> MediaPacket {
        let payload = Bytes::from_static(b"synthetic-frame");
        let mut header = MediaPacketHeader::new(
            PacketType::Video,
            CodecId::H264,
            stream_id,
            1,
            payload.len() as u16,
        );
        header.frame_id = 1;
        header.flags = PacketFlags::empty().with(PacketFlags::END_OF_FRAME);
        MediaPacket { header, payload }
    }

    async fn send_control_request(
        connection: &Connection,
        request: ClientEnvelope,
    ) -> ServerEnvelope {
        let (mut send, mut recv) = connection.open_bi().await.expect("control stream opens");
        send.write_all(&encode_client_envelope(&request).unwrap())
            .await
            .expect("request writes");
        send.finish().expect("request finishes");
        let response_bytes = recv
            .read_to_end(CONTROL_STREAM_READ_LIMIT)
            .await
            .expect("response reads");
        decode_server_envelope(&response_bytes).unwrap()
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
