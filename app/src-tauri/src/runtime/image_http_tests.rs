//! Real native-TLS adapter tests. All certificate/key material exists only in memory.
#[path = "image_acquire_tests.rs"]
mod acquisition;
#[path = "image_http_deadline_tests.rs"]
mod deadlines;
#[path = "image_http_redirect_tests.rs"]
mod redirects;
use super::*;
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::watch,
};
use tokio_rustls::{
    rustls::{
        self,
        pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
    },
    TlsAcceptor,
};

struct Server {
    address: std::net::SocketAddr,
    root: Vec<u8>,
    calls: Arc<AtomicUsize>,
    routes: Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn start(response: &[u8]) -> Self {
        let mut response = response.to_vec();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.not_before = (std::time::SystemTime::now() - Duration::from_secs(60)).into();
        params.not_after = (std::time::SystemTime::now() + Duration::from_secs(86400)).into();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let root_key = KeyPair::generate().unwrap();
        params.distinguished_name.push(
            rcgen::DnType::CommonName,
            format!("Memory test CA {}", &hash(root_key.public_key_raw())[..16]),
        );
        let root = params.self_signed(&root_key).unwrap().der().to_vec();
        let issuer = Issuer::new(params, root_key);
        let mut params = CertificateParams::new(vec!["image.invalid".into()]).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "image.invalid");
        params.not_before = (std::time::SystemTime::now() - Duration::from_secs(60)).into();
        params.not_after = (std::time::SystemTime::now() + Duration::from_secs(86400)).into();
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        let leaf_key = KeyPair::generate().unwrap();
        let leaf = params.signed_by(&leaf_key, &issuer).unwrap();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![leaf.der().clone()], key)
        .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        if let Some(offset) = response.windows(6).position(|bytes| bytes == b"{port}") {
            response.splice(offset..offset + 6, address.port().to_string().bytes());
        }
        let routes = Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            String,
            Vec<u8>,
        >::new()));
        let server_routes = routes.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                if response == b"handshake-stall" {
                    let _socket = socket;
                    std::future::pending::<()>().await;
                    continue;
                }
                let Ok(mut tls) = acceptor.accept(socket).await else {
                    continue;
                };
                let mut header = Vec::new();
                let mut byte = [0];
                while header.len() < 4096 && !header.ends_with(b"\r\n\r\n") {
                    if tls.read_exact(&mut byte).await.is_err() {
                        break;
                    }
                    header.push(byte[0]);
                }
                // Some native TLS stacks finish the handshake before rejecting the peer.
                if header.is_empty() {
                    continue;
                }
                observed.fetch_add(1, Ordering::SeqCst);
                assert!(String::from_utf8_lossy(&header)
                    .to_lowercase()
                    .contains("accept-encoding: identity\r\n"));
                if response == b"stall" || response == b"body-stall" || response == b"partial-stall"
                {
                    if response != b"stall" {
                        tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n")
                            .await
                            .unwrap();
                        if response == b"partial-stall" {
                            tls.write_all(b"ti").await.unwrap();
                        }
                        tls.flush().await.unwrap();
                    }
                    std::future::pending::<()>().await;
                } else {
                    let path = String::from_utf8_lossy(&header)
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .to_owned();
                    let mut response = server_routes
                        .lock()
                        .unwrap()
                        .get(&path)
                        .cloned()
                        .unwrap_or_else(|| response.clone());
                    // This fixture serves one request per connection. Advertise
                    // that before replying so native clients cannot pool a
                    // connection that is already being closed.
                    let end = response.windows(4).position(|b| b == b"\r\n\r\n").unwrap();
                    response.splice(end..end, b"\r\nConnection: close".iter().copied());
                    let _ = tls.write_all(&response).await;
                    let _ = tls.shutdown().await;
                }
            }
        });
        Self {
            address,
            root,
            calls,
            routes,
            task,
        }
    }
    fn client(&self, trusted: bool, host: &str) -> ImageHttpClient {
        self.client_with_timeout(trusted, host, Some(Duration::from_millis(500)))
    }
    fn production_client(&self) -> ImageHttpClient {
        self.client_with_timeout(true, "image.invalid", None)
    }
    fn client_with_timeout(
        &self,
        trusted: bool,
        host: &str,
        idle: Option<Duration>,
    ) -> ImageHttpClient {
        use super::super::image_types::{ImageTarget, TransferLimits};
        let url = format!("https://{host}:{}/", self.address.port());
        let pin = ImagePin {
            release_id: "nonbootable-test".into(),
            manifest_url: url.clone(),
            manifest_sha256: hash(b"tiny"),
            accepted_schema_sha256: "a".repeat(64),
            target: ImageTarget::WindowsAmd64,
            asset_base: url,
            redirect_origins: vec![],
            limits: TransferLimits {
                compressed_bytes: 4096,
                disk_bytes: 4096,
                kernel_bytes: 4096,
                initrd_bytes: 4096,
            },
        };
        let mut client = ImageHttpClient::new(&pin).unwrap();
        let mut builder = client_builder().resolve(host, self.address);
        if let Some(idle) = idle {
            builder = builder.read_timeout(idle);
        }
        if trusted {
            builder =
                builder.add_root_certificate(reqwest::Certificate::from_der(&self.root).unwrap());
        }
        client.client = builder.build().unwrap();
        client
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
async fn receive(client: &ImageHttpClient, length: u64) -> Result<Vec<u8>, ImageError> {
    let (_sender, closed) = watch::channel(false);
    let mut output = Vec::new();
    client
        .transfer(
            &client.pin.manifest_url,
            &mut output,
            length,
            (Some(length), &hash(b"tiny")),
            closed,
            |_| {},
        )
        .await?;
    Ok(output)
}
#[tokio::test]
async fn memory_ca_proves_native_trust_and_hostname_validation() {
    let server = Server::start(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ntiny").await;
    assert_eq!(
        receive(&server.client(true, "image.invalid"), 4)
            .await
            .unwrap(),
        b"tiny"
    );
    assert!(matches!(
        receive(&server.client(false, "image.invalid"), 4).await,
        Err(ImageError::Network(_))
    ));
    assert!(matches!(
        receive(&server.client(true, "wrong.invalid"), 4).await,
        Err(ImageError::Network(_))
    ));
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn real_https_refuses_encoding_length_hash_and_status_mismatches() {
    for (response, expected) in [
        (
            b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\n\r\ntiny"
                .as_slice(),
            ImageError::Integrity,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\ntiny!",
            ImageError::LimitExceeded,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nbads",
            ImageError::Integrity,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nti",
            ImageError::Network("image body"),
        ),
        (
            b"HTTP/1.1 503 Busy\r\nContent-Length: 4\r\n\r\ntiny",
            ImageError::Network("image status"),
        ),
    ] {
        let server = Server::start(response).await;
        assert_eq!(
            receive(&server.client(true, "image.invalid"), 4).await,
            Err(expected)
        );
        assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn real_https_timeout_and_cancellation_stop_without_retry() {
    let server = Server::start(b"stall").await;
    let client = server.client(true, "image.invalid");
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), receive(&client, 4))
            .await
            .unwrap(),
        Err(ImageError::Network(_))
    ));
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    let (sender, closed) = watch::channel(true);
    let mut output = Vec::new();
    assert_eq!(
        client
            .transfer(
                &client.pin.manifest_url,
                &mut output,
                4,
                (Some(4), &hash(b"tiny")),
                closed,
                |_| {}
            )
            .await,
        Err(ImageError::Cancelled)
    );
    drop(sender);
    assert!(output.is_empty());
}
