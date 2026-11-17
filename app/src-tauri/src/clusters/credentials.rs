//! Native-only credential resolution. A configured plugin is trusted code, not a sandbox.
use super::{AccessOutcome, Check, CheckResult};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::{
    client::{Body, ConfigExt},
    config::{
        AuthInfo, ExecAuthCluster, ExecConfig, ExecInteractiveMode, KubeConfigOptions, Kubeconfig,
    },
    Config,
};
use serde::Deserialize;
use std::{
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::{oneshot, watch},
    task::JoinHandle,
    time::Instant,
};
use tower::{Layer, ServiceExt};

#[cfg(all(test, unix))]
#[path = "credential_spawn_tests.rs"]
mod spawn_tests;

const LIMIT: Duration = Duration::from_secs(10);
/// Deliberately neither Debug nor Serialize: these values never cross desktop IPC.
pub enum Credential {
    Bearer {
        value: String,
        known_expiry: Option<Time>,
    },
    CertificateOnly,
}
pub struct ResolvedCreator {
    pub config: Config,
    pub credential: Credential,
}
#[derive(Clone)]
pub(super) struct ExecOwner(Arc<Owner>);
struct Owner {
    stop: watch::Sender<bool>,
    jobs: Mutex<Vec<JoinHandle<()>>>,
}
impl Default for ExecOwner {
    fn default() -> Self {
        Self(Arc::new(Owner {
            stop: watch::channel(false).0,
            jobs: Mutex::new(Vec::new()),
        }))
    }
}
impl ExecOwner {
    pub(super) async fn shutdown(&self) {
        let jobs = {
            let mut jobs = self.0.jobs.lock().unwrap();
            self.0.stop.send_replace(true);
            std::mem::take(&mut *jobs)
        };
        for job in jobs {
            let _ = job.await;
        }
    }
    async fn execute(&self, exec: ExecConfig, deadline: Instant) -> Result<Output, AccessOutcome> {
        let (send, receive) = oneshot::channel();
        {
            let mut jobs = self.0.jobs.lock().unwrap();
            if *self.0.stop.borrow() {
                return Err(AccessOutcome::Unavailable);
            }
            // Ownership outlives an IPC waiter; shutdown cancels and joins every direct child.
            let stop = self.0.stop.subscribe();
            jobs.retain(|job| !job.is_finished());
            jobs.push(tokio::spawn(async move {
                let _ = send.send(execute(exec, deadline, stop).await);
            }));
        }
        receive.await.map_err(|_| AccessOutcome::Unavailable)?
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Output {
    kind: String,
    api_version: String,
    status: Status,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    token: Option<String>,
    client_certificate_data: Option<String>,
    client_key_data: Option<String>,
    expiration_timestamp: Option<Time>,
}
async fn bounded(reader: impl AsyncRead + Unpin, limit: u64) -> Result<Vec<u8>, AccessOutcome> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| AccessOutcome::Unavailable)?;
    if bytes.len() as u64 > limit {
        return Err(AccessOutcome::Unavailable);
    }
    Ok(bytes)
}
async fn execute(
    exec: ExecConfig,
    deadline: Instant,
    mut stop: watch::Receiver<bool>,
) -> Result<Output, AccessOutcome> {
    let deadline = deadline.min(Instant::now() + LIMIT);
    let version = exec
        .api_version
        .as_deref()
        .ok_or(AccessOutcome::UnsupportedCredential)?;
    if !matches!(
        version,
        "client.authentication.k8s.io/v1" | "client.authentication.k8s.io/v1beta1"
    ) || exec.interactive_mode == Some(ExecInteractiveMode::Always)
        || (version.ends_with("/v1") && exec.interactive_mode.is_none())
    {
        return Err(AccessOutcome::UnsupportedCredential);
    }
    let command = exec
        .command
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or(AccessOutcome::UnsupportedCredential)?;
    let mut cmd = Command::new(command);
    cmd.args(exec.args.as_deref().unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    for env in exec.env.as_deref().unwrap_or_default() {
        cmd.env(
            env.get("name").ok_or(AccessOutcome::Unavailable)?,
            env.get("value").ok_or(AccessOutcome::Unavailable)?,
        );
    }
    let mut spec = serde_json::json!({"interactive":false});
    if exec.provide_cluster_info {
        spec["cluster"] =
            serde_json::to_value(&exec.cluster).map_err(|_| AccessOutcome::Unavailable)?;
    }
    cmd.env(
        "KUBERNETES_EXEC_INFO",
        serde_json::json!({"kind":"ExecCredential","apiVersion":version,"spec":spec}).to_string(),
    );
    for name in exec.drop_env.as_deref().unwrap_or_default() {
        cmd.env_remove(name);
    }
    if *stop.borrow() || deadline <= Instant::now() {
        return Err(AccessOutcome::Unavailable);
    }
    let spawn = crate::runtime::spawn_guard::run(|| cmd.spawn());
    let mut child = tokio::select! {
        biased;
        _ = stop.changed() => return Err(AccessOutcome::Unavailable),
        _ = tokio::time::sleep_until(deadline) => return Err(AccessOutcome::Unavailable),
        result = spawn => result.map_err(|_| AccessOutcome::Unavailable)?,
    };
    let stdout = child.stdout.take().ok_or(AccessOutcome::Unavailable)?;
    let stderr = child.stderr.take().ok_or(AccessOutcome::Unavailable)?;
    let result = tokio::select! {
        biased;
        _ = stop.changed() => Err(AccessOutcome::Unavailable),
        _ = tokio::time::sleep_until(deadline) => Err(AccessOutcome::Unavailable),
        result = async {
            let (stdout, _, status) = tokio::try_join!(bounded(stdout, 1024*1024), bounded(stderr, 64*1024), async { child.wait().await.map_err(|_| AccessOutcome::Unavailable) })?;
            if !status.success() { return Err(AccessOutcome::Unavailable); }
            serde_json::from_slice::<Output>(&stdout).map_err(|_| AccessOutcome::Unavailable)
        } => result,
    };
    // Dropping pipes bounds descendants holding them open. Only the direct child is killed/reaped.
    if result.is_err() {
        let _ = child.start_kill();
    }
    let _ = child.wait().await;
    let output = result?;
    if output.kind != "ExecCredential" || output.api_version != version {
        return Err(AccessOutcome::Unavailable);
    }
    Ok(output)
}
/// Binding-only and token-only construction never resolves another credential or executes a plugin.
pub(super) async fn selected_config(
    kubeconfig: &Kubeconfig,
    name: &str,
    token: &str,
) -> Result<Config, AccessOutcome> {
    let mut copy = kubeconfig.clone();
    let (user, _) = selected(&mut copy, name)?;
    *user = AuthInfo {
        token: Some(token.to_owned().into()),
        ..Default::default()
    };
    configured(copy, name).await
}
fn selected<'a>(
    cfg: &'a mut Kubeconfig,
    name: &str,
) -> Result<(&'a mut AuthInfo, kube::config::Cluster), AccessOutcome> {
    let context = cfg
        .contexts
        .iter()
        .find(|c| c.name == name)
        .and_then(|c| c.context.as_ref())
        .ok_or(Check::Configuration.unknown())?;
    let cluster = cfg
        .clusters
        .iter()
        .find(|c| c.name == context.cluster)
        .and_then(|c| c.cluster.clone())
        .ok_or(Check::Configuration.unknown())?;
    let user = cfg
        .auth_infos
        .iter_mut()
        .find(|u| Some(&u.name) == context.user.as_ref())
        .and_then(|u| u.auth_info.as_mut())
        .ok_or(Check::Credentials.fail(CheckResult::UnsupportedCredential))?;
    Ok((user, cluster))
}
async fn configured(copy: Kubeconfig, name: &str) -> Result<Config, AccessOutcome> {
    configured_config(copy, name)
        .await
        .map_err(|e| e.at(Check::Configuration))
}
async fn configured_config(copy: Kubeconfig, name: &str) -> Result<Config, AccessOutcome> {
    let proxy = {
        let context = copy
            .contexts
            .iter()
            .find(|c| c.name == name)
            .and_then(|c| c.context.as_ref())
            .ok_or(AccessOutcome::Unavailable)?;
        copy.clusters
            .iter()
            .find(|c| c.name == context.cluster)
            .and_then(|c| c.cluster.as_ref())
            .and_then(|c| c.proxy_url.clone())
    };
    let mut config = Config::from_custom_kubeconfig(
        copy,
        &KubeConfigOptions {
            context: Some(name.into()),
            ..Default::default()
        },
    )
    .await
    .map_err(|_| AccessOutcome::Unavailable)?;
    if let Some(proxy) = proxy {
        config.proxy_url = Some(proxy.parse().map_err(|_| AccessOutcome::Unavailable)?);
    }
    config.default_retry = false;
    config.connect_timeout = Some(LIMIT);
    config.read_timeout = Some(LIMIT);
    config.write_timeout = Some(LIMIT);
    config.reset_reader_on_write = true;
    Ok(config)
}
pub(super) async fn resolve(
    kubeconfig: &Kubeconfig,
    name: &str,
    deadline: Instant,
    owner: &ExecOwner,
) -> Result<ResolvedCreator, AccessOutcome> {
    resolve_creator(kubeconfig, name, deadline, owner)
        .await
        .map_err(|e| e.at(Check::Credentials))
}
async fn resolve_creator(
    kubeconfig: &Kubeconfig,
    name: &str,
    deadline: Instant,
    owner: &ExecOwner,
) -> Result<ResolvedCreator, AccessOutcome> {
    if *owner.0.stop.borrow() {
        return Err(AccessOutcome::Unavailable);
    }
    let mut copy = kubeconfig.clone();
    let (user, cluster) = selected(&mut copy, name)?;
    if user.auth_provider.is_some() || (user.username.is_some() && user.password.is_some()) {
        return Err(AccessOutcome::UnsupportedCredential);
    }
    let bearer = user.token.is_some() || user.token_file.is_some();
    let certificate = (user.client_certificate.is_some() || user.client_certificate_data.is_some())
        && (user.client_key.is_some() || user.client_key_data.is_some());
    let mut expiry = None;
    if !bearer {
        if let Some(mut exec) = user.exec.take() {
            // A mixed certificate/exec config is ambiguous; preserve the thin path's rejection.
            if certificate {
                return Err(AccessOutcome::UnsupportedCredential);
            }
            if exec.provide_cluster_info {
                exec.cluster = Some(
                    ExecAuthCluster::try_from(&cluster).map_err(|_| AccessOutcome::Unavailable)?,
                );
            }
            let status = owner.execute(exec, deadline).await?.status;
            expiry = status.expiration_timestamp;
            if expiry
                .as_ref()
                .is_some_and(|t| t.0 <= k8s_openapi::jiff::Timestamp::now())
            {
                return Err(AccessOutcome::Unavailable);
            }
            // Impersonation constrains creator Kubernetes requests; it is not a competing credential.
            *user = AuthInfo {
                impersonate: user.impersonate.take(),
                impersonate_uid: user.impersonate_uid.take(),
                impersonate_groups: user.impersonate_groups.take(),
                impersonate_user_extra: user.impersonate_user_extra.take(),
                ..Default::default()
            };
            let token = status.token.filter(|s| !s.is_empty());
            let cert = status.client_certificate_data.filter(|s| !s.is_empty());
            let key = status.client_key_data.filter(|s| !s.is_empty());
            if token.is_none() && cert.is_none() && key.is_none() {
                return Err(AccessOutcome::Unavailable);
            }
            if let Some(token) = token {
                http::HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| AccessOutcome::Unavailable)?;
                user.token = Some(token.into());
            }
            match (cert, key) {
                (None, None) => {}
                (Some(cert), Some(key)) => {
                    use base64::Engine;
                    user.client_certificate_data =
                        Some(base64::engine::general_purpose::STANDARD.encode(cert));
                    user.client_key_data =
                        Some(base64::engine::general_purpose::STANDARD.encode(key).into());
                }
                _ => return Err(AccessOutcome::Unavailable),
            }
        } else if !certificate {
            return Err(AccessOutcome::UnsupportedCredential);
        }
    }
    user.exec = None;
    let mut config = configured(copy, name).await?;
    let credential = if let Some(value) = capture_bearer(&config).await? {
        config.auth_info.token = Some(value.clone().into());
        config.auth_info.token_file = None;
        Credential::Bearer {
            value,
            known_expiry: expiry,
        }
    } else {
        Credential::CertificateOnly
    };
    Ok(ResolvedCreator { config, credential })
}
async fn capture_bearer(config: &Config) -> Result<Option<String>, AccessOutcome> {
    let Some(layer) = config
        .auth_layer()
        .map_err(|_| AccessOutcome::Unavailable)?
    else {
        return Ok(None);
    };
    // No Kubernetes URI, headers or TLS: capture the public auth layer on a non-network sink.
    let sink = tower::service_fn(|request: http::Request<Body>| async move {
        Ok::<_, tower::BoxError>(http::Response::new(
            request.headers().get(http::header::AUTHORIZATION).cloned(),
        ))
    });
    let header = layer
        .layer(sink)
        .boxed()
        .oneshot(http::Request::new(Body::empty()))
        .await
        .map_err(|_| AccessOutcome::Unavailable)?
        .into_body();
    let value = header
        .as_ref()
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if header.is_some() && value.is_none() {
        return Err(AccessOutcome::UnsupportedCredential);
    }
    Ok(value)
}
