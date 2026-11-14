use super::*;
use crate::engine::run::{ErrorKind, RunFuture, RunLimits, RunTransport};
use http::{header, HeaderValue, Request};
use http_body_util::{BodyExt, Full};
use hyper::{body::Bytes, client::conn::http1};
use hyper_util::rt::TokioIo;
use k8s_openapi::{
    api::core::v1::{Pod, Service},
    apimachinery::pkg::util::intstr::IntOrString,
};
use kube::{
    api::{ListParams, Portforwarder},
    Api, Client, ResourceExt,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::Mutex as AsyncMutex,
    task::JoinHandle,
    time::{timeout_at, Instant},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CAP: usize = 1024 * 1024;

/// GET-only access checks use the same stream protocol as runs, never a host listener.
pub struct PodOrkaRead;
impl OrkaRead for PodOrkaRead {
    fn get<'a>(
        &'a self,
        creator: &'a Client,
        binding: &'a TargetBinding,
        bearer: &'a str,
        resource: OrkaResource,
        namespace: &'a str,
        after: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Value, AccessOutcome>> + Send + 'a>> {
        Box::pin(async move {
            let deadline = Instant::now() + REQUEST_TIMEOUT;
            Check::OrkaConnection.begin();
            let connection = Connection::open(creator, binding, deadline)
                .await
                .map_err(|e| orka_error(Check::OrkaConnection, e))?;
            resource.check().begin();
            let result = connection
                .request(
                    path(resource),
                    namespace,
                    bearer,
                    &[bearer],
                    after,
                    None,
                    deadline,
                    resource == OrkaResource::AgentInventory,
                )
                .await;
            connection.close().await;
            result.map_err(|e| orka_error(resource.check(), e))
        })
    }
}
fn path(resource: OrkaResource) -> &'static str {
    match resource {
        OrkaResource::WhoAmI => "auth/whoami",
        OrkaResource::Agents | OrkaResource::AgentInventory => "agents",
        OrkaResource::Providers => "providers",
        OrkaResource::Tools => "tools",
    }
}
/// Send's rechecks reuse the attempt's opened stream (same binding); its owner closes it.
impl OrkaRead for OrkaStreamTransport {
    fn get<'a>(
        &'a self,
        _: &'a Client,
        _: &'a TargetBinding,
        bearer: &'a str,
        resource: OrkaResource,
        namespace: &'a str,
        after: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Value, AccessOutcome>> + Send + 'a>> {
        Box::pin(async move {
            let deadline = Instant::now() + REQUEST_TIMEOUT;
            self.connection
                .request(
                    path(resource),
                    namespace,
                    bearer,
                    &[bearer],
                    after,
                    None,
                    deadline,
                    resource == OrkaResource::AgentInventory,
                )
                .await
                .map_err(|e| orka_error(resource.check(), e))
        })
    }
}
/// Orka's own status codes; never relabelled as a Kubernetes permission check.
fn orka_error(check: Check, error: ErrorKind) -> AccessOutcome {
    check.fail(match error {
        ErrorKind::Authentication => CheckResult::AuthenticationRejected,
        ErrorKind::Permission => CheckResult::PermissionDenied,
        _ => CheckResult::Unknown,
    })
}

/// One opened Pod stream for the attempt. Never reconnect, reselect or retry a write.
pub struct OrkaStreamTransport {
    connection: Connection,
    namespace: String,
    provider: String,
    bearer: String,
    creator_bearer: String,
    current: ClusterClient,
    creator: Client,
    binding: TargetBinding,
    read_only: bool,
}
impl OrkaStreamTransport {
    pub fn peer(&self) -> &ForwardPeer {
        &self.connection.peer
    }
}
impl RemoteAccess {
    /// `current` must be freshly read at admission, not a cached context-name check.
    pub async fn transport(
        &self,
        current: &ClusterClient,
        provider: ProviderId,
    ) -> Result<OrkaStreamTransport, ClusterError> {
        let provider = self
            .providers()
            .iter()
            .find(|p| p.id == provider)
            .ok_or(ClusterError::Unavailable)?;
        self.open_transport(current, provider.name.clone(), false)
            .await
    }
    /// Historical observation needs no current Provider and never permits a POST.
    pub(crate) async fn read_transport(
        &self,
        current: &ClusterClient,
    ) -> Result<OrkaStreamTransport, ClusterError> {
        self.open_transport(current, String::new(), true).await
    }
    async fn open_transport(
        &self,
        current: &ClusterClient,
        provider: String,
        read_only: bool,
    ) -> Result<OrkaStreamTransport, ClusterError> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        timeout_at(deadline, current.revalidate(self))
            .await
            .map_err(|_| ClusterError::Unavailable)??;
        let connection = Connection::open(&self.creator, &self.binding, deadline)
            .await
            .map_err(|e| match e {
                ErrorKind::Authentication | ErrorKind::Permission => ClusterError::Denied,
                _ => ClusterError::Unavailable,
            })?;
        Ok(OrkaStreamTransport {
            connection,
            namespace: self.binding.watched_namespace.clone(),
            provider,
            read_only,
            bearer: self.bearer.clone(),
            creator_bearer: self.creator_bearer.clone(),
            current: current.clone(),
            creator: self.creator.clone(),
            binding: self.binding.clone(),
        })
    }
}
impl RunTransport for OrkaStreamTransport {
    fn namespace(&self) -> &str {
        &self.namespace
    }
    fn provider(&self) -> &str {
        &self.provider
    }
    fn limits(&self) -> RunLimits {
        RunLimits::default()
    }
    fn close(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.connection.close())
    }
    fn request<'a>(
        &'a self,
        path: &'a str,
        after: Option<u64>,
        body: Option<&'a Value>,
        deadline: Instant,
    ) -> RunFuture<'a> {
        Box::pin(async move {
            let deadline = deadline.min(Instant::now() + REQUEST_TIMEOUT);
            if body.is_some() {
                if self.read_only {
                    return Err(ErrorKind::Configuration);
                }
                let validation = timeout_at(
                    deadline,
                    self.current
                        .revalidate_binding(&self.binding, &self.creator),
                )
                .await;
                if !matches!(validation, Ok(Ok(()))) {
                    self.connection.close().await;
                    return Err(match validation {
                        Ok(Err(ClusterError::Denied)) => ErrorKind::Permission,
                        _ => ErrorKind::Identity,
                    });
                }
            }
            let after = after.map(|a| a.to_string());
            self.connection
                .request(
                    path,
                    &self.namespace,
                    &self.bearer,
                    &[&self.bearer, &self.creator_bearer],
                    after.as_deref(),
                    body,
                    deadline,
                    false,
                )
                .await
        })
    }
}

// Own the forward immediately: cancellation during handshake must not detach it.
#[derive(Default)]
struct Drivers {
    forward: Option<Portforwarder>,
    http: Option<JoinHandle<()>>,
}
impl Drivers {
    fn abort(&self) {
        if let Some(forward) = &self.forward {
            forward.abort();
        }
        if let Some(http) = &self.http {
            http.abort();
        }
    }
    async fn close(mut self) {
        self.abort();
        if let Some(http) = self.http.take() {
            let _ = http.await;
        }
        if let Some(forward) = self.forward.take() {
            let _ = forward.join().await;
        }
    }
}
impl Drop for Drivers {
    fn drop(&mut self) {
        self.abort();
    }
}
struct Connection {
    peer: ForwardPeer,
    sender: AsyncMutex<Option<http1::SendRequest<Full<Bytes>>>>,
    drivers: Mutex<Drivers>,
    closing: AsyncMutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
}
impl Connection {
    async fn open(
        creator: &Client,
        binding: &TargetBinding,
        deadline: Instant,
    ) -> Result<Self, ErrorKind> {
        timeout_at(deadline, async {
            let peer = select_peer(creator, binding).await?;
            // Kubernetes forwards by name without a UID precondition; peer UID
            // is discovery metadata, not authenticated stream attestation.
            let forward = Api::<Pod>::namespaced(creator.clone(), &peer.pod_namespace)
                .portforward(&peer.pod_name, &[peer.port])
                .await
                .map_err(kube_error)?;
            let mut drivers = Drivers {
                forward: Some(forward),
                http: None,
            };
            let forward = drivers.forward.as_mut().unwrap();
            let stream = forward.take_stream(peer.port).ok_or(ErrorKind::Transport)?;
            let error = forward.take_error(peer.port).ok_or(ErrorKind::Transport)?;
            let (sender, connection) = http1::handshake(TokioIo::new(stream))
                .await
                .map_err(|_| ErrorKind::Transport)?;
            drivers.http = Some(tokio::spawn(async move {
                // Error-channel resolution invalidates the port; never export its message.
                tokio::select! { _ = connection => {}, _ = error => {} }
            }));
            Ok(Self {
                peer,
                sender: AsyncMutex::new(Some(sender)),
                drivers: Mutex::new(drivers),
                closing: AsyncMutex::new(None),
                closed: AtomicBool::new(false),
            })
        })
        .await
        .map_err(|_| ErrorKind::Deadline)?
    }
    async fn close(&self) {
        let mut closing = self.closing.lock().await;
        if !self.closed.swap(true, Ordering::SeqCst) {
            let drivers = std::mem::take(&mut *self.drivers.lock().unwrap());
            drivers.abort();
            // Stop can cancel an error-cleanup waiter. Keep the join job owned
            // here, and await it by reference so the next close can finish it.
            *closing = Some(tokio::spawn(drivers.close()));
        }
        if let Some(cleanup) = closing.as_mut() {
            let _ = cleanup.await;
        }
        closing.take();
        self.sender.lock().await.take();
    }
    #[allow(clippy::too_many_arguments)]
    async fn request(
        &self,
        path: &str,
        namespace: &str,
        bearer: &str,
        known: &[&str],
        after: Option<&str>,
        body: Option<&Value>,
        deadline: Instant,
        inventory: bool,
    ) -> Result<Value, ErrorKind> {
        let deadline = deadline.min(Instant::now() + REQUEST_TIMEOUT);
        let mut fully_read = false;
        let result = timeout_at(deadline, async {
            if self.closed.load(Ordering::SeqCst) {
                return Err(ErrorKind::Transport);
            }
            // URL is used only for encoding the origin-form path/query; it is never dialled.
            let mut url = reqwest::Url::parse("http://orka.invalid/").unwrap();
            url.set_path(&format!("/api/v1/{path}"));
            url.query_pairs_mut().append_pair("namespace", namespace);
            if path == "agents" && body.is_none() {
                url.query_pairs_mut()
                    .append_pair("limit", if inventory { "10" } else { "1" });
            }
            if let Some(after) = after {
                let key = if path.ends_with("/events") {
                    "after"
                } else {
                    "continue"
                };
                url.query_pairs_mut().append_pair(key, after);
                if path != "agents" {
                    url.query_pairs_mut().append_pair("limit", "1000");
                }
            }
            let uri = format!("{}?{}", url.path(), url.query().unwrap());
            let mut authorization = HeaderValue::from_str(&format!("Bearer {bearer}"))
                .map_err(|_| ErrorKind::Configuration)?;
            authorization.set_sensitive(true);
            let request = Request::builder()
                .method(if body.is_some() { "POST" } else { "GET" })
                .uri(uri)
                .header(header::HOST, "orka")
                .header(header::AUTHORIZATION, authorization)
                .header(header::ACCEPT_ENCODING, "identity")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(
                    body.map(serde_json::to_vec)
                        .transpose()
                        .map_err(|_| ErrorKind::Source)?
                        .unwrap_or_default(),
                )))
                .map_err(|_| ErrorKind::Configuration)?;
            let mut sender = self.sender.lock().await;
            let sender = sender.as_mut().ok_or(ErrorKind::Transport)?;
            sender.ready().await.map_err(|_| ErrorKind::Transport)?;
            let response = sender
                .send_request(request)
                .await
                .map_err(|_| ErrorKind::Transport)?;
            let status = response.status().as_u16();
            match status {
                401 => return Err(ErrorKind::Authentication),
                403 => return Err(ErrorKind::Permission),
                _ => {}
            }
            if response
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .is_some_and(|n| n > CAP as u64)
            {
                return Err(ErrorKind::Response);
            }
            let mut incoming = response.into_body();
            let mut bytes = Vec::new();
            while let Some(frame) = incoming.frame().await {
                let frame = frame.map_err(|_| ErrorKind::Transport)?;
                if let Ok(data) = frame.into_data() {
                    if data.len() > CAP - bytes.len() {
                        return Err(ErrorKind::Response);
                    }
                    bytes.extend_from_slice(&data);
                }
            }
            fully_read = true;
            if status == 404 && body.is_none() {
                return Err(ErrorKind::NotFound);
            }
            if status != if body.is_some() { 201 } else { 200 } {
                return Err(ErrorKind::Response);
            }
            let value = serde_json::from_slice(&bytes).map_err(|_| ErrorKind::Response)?;
            if known.iter().any(|b| super::client::echoed(&value, b)) {
                return Err(ErrorKind::Response);
            }
            Ok(value)
        })
        .await
        .unwrap_or(Err(ErrorKind::Deadline));
        // Missing optional evidence or a rejected, fully consumed JSON reply
        // must not poison a healthy lease. Never reuse an incomplete response.
        if result.is_err()
            && !(fully_read && matches!(result, Err(ErrorKind::Response | ErrorKind::NotFound)))
        {
            self.close().await;
        }
        result
    }
}
fn kube_error(error: kube::Error) -> ErrorKind {
    match error {
        kube::Error::Api(e) if e.code == 401 => ErrorKind::Authentication,
        kube::Error::Api(e) if e.code == 403 => ErrorKind::Permission,
        _ => ErrorKind::Transport,
    }
}
async fn select_peer(creator: &Client, binding: &TargetBinding) -> Result<ForwardPeer, ErrorKind> {
    let services = Api::<Service>::namespaced(creator.clone(), &binding.service_namespace)
        .list(&ListParams::default())
        .await
        .map_err(kube_error)?;
    let service = services
        .iter()
        .find(|s| {
            s.name_any() == binding.service_name && s.uid().as_deref() == Some(&binding.service_uid)
        })
        .ok_or(ErrorKind::Identity)?;
    let spec = service.spec.as_ref().ok_or(ErrorKind::Response)?;
    let selector = spec
        .selector
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or(ErrorKind::Response)?;
    let port = spec
        .ports
        .as_ref()
        .and_then(|ports| ports.iter().find(|p| p.name.as_deref() == Some("api")))
        .ok_or(ErrorKind::Response)?;
    let labels = selector
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    let pods = Api::<Pod>::namespaced(creator.clone(), &binding.service_namespace)
        .list(&ListParams::default().labels(&labels))
        .await
        .map_err(kube_error)?;
    for pod in pods {
        if pod.metadata.deletion_timestamp.is_some()
            || !selector
                .iter()
                .all(|(k, v)| pod.metadata.labels.as_ref().and_then(|l| l.get(k)) == Some(v))
        {
            continue;
        }
        if !pod.status.as_ref().is_some_and(|s| {
            s.phase.as_deref() == Some("Running")
                && s.conditions
                    .as_ref()
                    .is_some_and(|c| c.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
        }) {
            continue;
        }
        let number = match &port.target_port {
            Some(IntOrString::Int(n)) => Some(*n),
            None => Some(port.port),
            Some(IntOrString::String(name)) => pod
                .spec
                .as_ref()
                .and_then(|s| {
                    s.containers
                        .iter()
                        .flat_map(|c| c.ports.iter().flatten())
                        .find(|p| {
                            p.name.as_ref() == Some(name)
                                && p.protocol.as_deref().is_none_or(|p| p == "TCP")
                        })
                })
                .map(|p| p.container_port),
        };
        let Some(port) = number
            .and_then(|n| u16::try_from(n).ok())
            .filter(|p| *p > 0)
        else {
            continue;
        };
        if let (Some(uid), Some(name)) = (pod.uid(), pod.metadata.name) {
            return Ok(ForwardPeer {
                pod_name: name,
                pod_namespace: binding.service_namespace.clone(),
                pod_uid: uid,
                port,
            });
        }
    }
    Err(ErrorKind::Transport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[tokio::test]
    async fn waits_for_sender_readiness_without_closing_a_healthy_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (client, mut server) = tokio::io::duplex(8192);
        let (mut sender, driver) = http1::handshake(TokioIo::new(client)).await.unwrap();
        // Fill Hyper's one initial queue slot before its driver is polled.
        let first = sender.send_request(
            Request::builder()
                .uri("/first")
                .header(header::HOST, "orka")
                .body(Full::new(Bytes::new()))
                .unwrap(),
        );
        assert!(!sender.is_ready());
        let (release, gate) = tokio::sync::oneshot::channel();
        let driver = tokio::spawn(async move {
            let _ = gate.await;
            let _ = driver.await;
        });
        let connection = Connection {
            peer: ForwardPeer {
                pod_name: "fixture".into(),
                pod_namespace: "fixture".into(),
                pod_uid: "fixture".into(),
                port: 80,
            },
            sender: AsyncMutex::new(Some(sender)),
            drivers: Mutex::new(Drivers {
                forward: None,
                http: Some(driver),
            }),
            closing: AsyncMutex::new(None),
            closed: AtomicBool::new(false),
        };
        let mut next = Box::pin(connection.request(
            "agents",
            "agents",
            "synthetic",
            &["synthetic"],
            None,
            None,
            Instant::now() + Duration::from_secs(2),
            false,
        ));
        std::future::poll_fn(|cx| {
            assert!(next.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert!(
            !connection.closed.load(Ordering::SeqCst),
            "a temporarily unready sender must not close the lease"
        );
        release.send(()).unwrap();
        let responder = tokio::spawn(async move {
            for _ in 0..2 {
                let mut bytes = Vec::new();
                loop {
                    let mut byte = [0];
                    server.read_exact(&mut byte).await.unwrap();
                    bytes.push(byte[0]);
                    if bytes.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                server
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                    .await
                    .unwrap();
            }
        });
        let consume_first = async {
            first.await.unwrap().into_body().collect().await.unwrap();
        };
        let (value, ()) = tokio::join!(next, consume_first);
        assert_eq!(value.unwrap(), serde_json::json!({}));
        responder.await.unwrap();
        connection.close().await;
    }
    #[tokio::test]
    async fn close_aborts_and_joins_drivers() {
        struct Exited(Arc<AtomicBool>);
        impl Drop for Exited {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let exited = Arc::new(AtomicBool::new(false));
        let guard = Exited(exited.clone());
        let driver = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        let connection = Connection {
            peer: ForwardPeer {
                pod_name: "fixture".into(),
                pod_namespace: "fixture".into(),
                pod_uid: "fixture".into(),
                port: 80,
            },
            sender: AsyncMutex::new(None),
            drivers: Mutex::new(Drivers {
                forward: None,
                http: Some(driver),
            }),
            closing: AsyncMutex::new(None),
            closed: AtomicBool::new(false),
        };
        // Reproduce Stop cancelling request-error cleanup at its first join.
        let mut interrupted = Box::pin(connection.close());
        std::future::poll_fn(|cx| {
            assert!(interrupted.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        drop(interrupted);
        tokio::join!(connection.close(), connection.close());
        // Unlike an abort-only check, Drop has already run when close returns.
        assert!(exited.load(Ordering::SeqCst));
        assert!(connection.drivers.lock().unwrap().http.is_none());
        assert!(connection.sender.lock().await.is_none());
    }
}
