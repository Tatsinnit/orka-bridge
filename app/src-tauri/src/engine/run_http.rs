use super::ErrorKind;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    Client, Method, Url,
};
use serde_json::Value;
use std::{net::IpAddr, time::Duration};
use tokio::time::{timeout_at, Instant};

/// Construct only from trusted native configuration, never webview input.
/// Deliberately neither Debug nor Serialize; there is no ambient auth or context.
pub struct NativeRunTarget {
    endpoint: Url,
    pub(super) namespace: String,
    pub(super) provider: String,
    bearer: Option<HeaderValue>,
}
fn dns(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}
impl NativeRunTarget {
    pub fn new(
        endpoint: &str,
        namespace: &str,
        provider: &str,
        bearer: Option<&str>,
    ) -> Result<Self, ErrorKind> {
        let endpoint = Url::parse(endpoint).map_err(|_| ErrorKind::Configuration)?;
        let loopback = endpoint
            .host_str()
            .and_then(|h| h.trim_matches(['[', ']']).parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        if !(endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback)
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.path() != "/"
            || !dns(namespace)
            || !dns(provider)
        {
            return Err(ErrorKind::Configuration);
        }
        let bearer = bearer
            .map(|b| {
                if b.is_empty() {
                    return Err(ErrorKind::Configuration);
                }
                let mut value = HeaderValue::from_str(&format!("Bearer {b}"))
                    .map_err(|_| ErrorKind::Configuration)?;
                value.set_sensitive(true);
                Ok(value)
            })
            .transpose()?;
        Ok(Self {
            endpoint,
            namespace: namespace.into(),
            provider: provider.into(),
            bearer,
        })
    }
}
#[derive(Clone, Copy)]
pub struct RunLimits {
    pub request_timeout: Duration,
    pub operation_timeout: Duration,
    pub poll_interval: Duration,
}
impl Default for RunLimits {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(10),
            operation_timeout: Duration::from_secs(150),
            poll_interval: Duration::from_millis(250),
        }
    }
}
pub type RunFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, ErrorKind>> + Send + 'a>>;
/// Native-owned request transport; implementations never accept renderer targets.
pub trait RunTransport: Send + Sync {
    fn namespace(&self) -> &str;
    fn provider(&self) -> &str;
    fn limits(&self) -> RunLimits;
    /// Transport-specific whole-create-body limit, not a source-format limit.
    fn max_create_body_bytes(&self) -> Option<usize> {
        None
    }
    /// Release owned connection tasks. Local transports need no asynchronous cleanup.
    fn close(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
    fn request<'a>(
        &'a self,
        path: &'a str,
        after: Option<u64>,
        body: Option<&'a Value>,
        deadline: Instant,
    ) -> RunFuture<'a>;
    fn deadline(&self) -> Instant {
        Instant::now() + self.limits().operation_timeout
    }
    fn pause(
        &self,
        deadline: Instant,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), ErrorKind>> + Send + '_>>
    {
        Box::pin(async move {
            timeout_at(deadline, tokio::time::sleep(self.limits().poll_interval))
                .await
                .map_err(|_| ErrorKind::Deadline)
        })
    }
}
pub(super) struct Http {
    pub target: NativeRunTarget,
    pub limits: RunLimits,
    client: Client,
}
impl Http {
    pub fn new(target: NativeRunTarget, limits: RunLimits) -> Result<Self, ErrorKind> {
        if limits.request_timeout.is_zero()
            || limits.request_timeout > Duration::from_secs(30)
            || limits.operation_timeout.is_zero()
            || limits.operation_timeout > Duration::from_secs(300)
            || limits.poll_interval.is_zero()
            || limits.poll_interval > limits.operation_timeout
        {
            return Err(ErrorKind::Configuration);
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()
            .map_err(|_| ErrorKind::Configuration)?;
        Ok(Self {
            target,
            limits,
            client,
        })
    }
    pub fn deadline(&self) -> Instant {
        Instant::now() + self.limits.operation_timeout
    }
    pub async fn request(
        &self,
        path: &str,
        after: Option<u64>,
        body: Option<&Value>,
        deadline: Instant,
    ) -> Result<Value, ErrorKind> {
        let deadline = deadline.min(Instant::now() + self.limits.request_timeout);
        timeout_at(deadline, async {
            let mut url = self.target.endpoint.clone();
            url.set_path(&format!("/api/v1/{path}"));
            url.query_pairs_mut()
                .append_pair("namespace", &self.target.namespace);
            if let Some(after) = after {
                url.query_pairs_mut()
                    .append_pair("after", &after.to_string())
                    .append_pair("limit", "1000");
            }
            let mut request = self
                .client
                .request(
                    if body.is_some() {
                        Method::POST
                    } else {
                        Method::GET
                    },
                    url,
                )
                .header("Accept-Encoding", "identity");
            if let Some(bearer) = &self.target.bearer {
                request = request.header(AUTHORIZATION, bearer);
            }
            if let Some(body) = body {
                request = request
                    .header(CONTENT_TYPE, "application/json")
                    .body(serde_json::to_vec(body).map_err(|_| ErrorKind::Source)?);
            }
            let mut response = request.send().await.map_err(|_| ErrorKind::Transport)?;
            match response.status().as_u16() {
                401 => return Err(ErrorKind::Authentication),
                403 => return Err(ErrorKind::Permission),
                404 if body.is_none() => return Err(ErrorKind::NotFound),
                status if status != if body.is_some() { 201 } else { 200 } => {
                    return Err(ErrorKind::Response)
                }
                _ => {}
            }
            const CAP: usize = 1024 * 1024;
            if response.content_length().is_some_and(|n| n > CAP as u64) {
                return Err(ErrorKind::Response);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| ErrorKind::Transport)? {
                if chunk.len() > CAP - bytes.len() {
                    return Err(ErrorKind::Response);
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).map_err(|_| ErrorKind::Response)
        })
        .await
        .map_err(|_| ErrorKind::Deadline)?
    }
}
impl RunTransport for Http {
    fn namespace(&self) -> &str {
        &self.target.namespace
    }
    fn provider(&self) -> &str {
        &self.target.provider
    }
    fn limits(&self) -> RunLimits {
        self.limits
    }
    fn deadline(&self) -> Instant {
        Http::deadline(self)
    }
    fn request<'a>(
        &'a self,
        path: &'a str,
        after: Option<u64>,
        body: Option<&'a Value>,
        deadline: Instant,
    ) -> RunFuture<'a> {
        Box::pin(Http::request(self, path, after, body, deadline))
    }
}
