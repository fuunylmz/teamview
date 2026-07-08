use std::{net::SocketAddr, sync::Arc};

use anyhow::Context;
use quinn::{Endpoint, ServerConfig};
use rcgen::generate_simple_self_signed;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

pub fn build_server_endpoint(listen_addr: &str) -> anyhow::Result<Endpoint> {
    let addr: SocketAddr = listen_addr
        .parse()
        .with_context(|| format!("invalid listen address {listen_addr}"))?;
    let server_config = build_server_config()?;
    Endpoint::server(server_config, addr).context("failed to bind QUIC endpoint")
}

fn build_server_config() -> anyhow::Result<ServerConfig> {
    let cert = generate_simple_self_signed(vec!["localhost".to_owned()])?;
    let cert_der = CertificateDer::from(cert.cert);
    let key_der = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()));
    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)?;
    server_crypto.alpn_protocols = vec![b"teamview-stage1".to_vec()];

    let mut server_config = ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
    ));
    let transport =
        Arc::get_mut(&mut server_config.transport).expect("transport is uniquely owned");
    transport.max_concurrent_bidi_streams(16_u32.into());
    transport.datagram_receive_buffer_size(Some(1_000_000));
    Ok(server_config)
}

#[cfg(test)]
mod tests {
    use quinn::ClientConfig;

    use super::*;

    #[test]
    fn rejects_invalid_listen_addr() {
        assert!(build_server_endpoint("not-an-addr").is_err());
    }

    #[tokio::test]
    async fn local_client_connects_to_server_endpoint() {
        let server = build_server_endpoint("127.0.0.1:0").unwrap();
        let server_addr = server.local_addr().unwrap();
        let accept_task = tokio::spawn(async move {
            let incoming = server.accept().await.expect("incoming connection");
            incoming.await.expect("accepted connection")
        });

        let mut client = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        client.set_default_client_config(build_insecure_local_client_config());
        let connection = client
            .connect(server_addr, "localhost")
            .unwrap()
            .await
            .expect("client connects");
        assert_eq!(connection.remote_address(), server_addr);

        let accepted = accept_task.await.unwrap();
        assert!(accepted.remote_address().port() > 0);
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
