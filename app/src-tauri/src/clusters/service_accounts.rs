//! Advisory, creator-authenticated reads only. Explicit Use remains the sole token path.
use super::*;
use http_body_util::BodyExt;
use kube::{client::Body, core::Request};

const CAP: usize = 1024 * 1024;

fn failure(error: AccessOutcome) -> ClusterFailure {
    error.failure(Check::ServiceAccount)
}

impl ClusterClient {
    async fn account_creator(
        &self,
        id: TargetId,
        deadline: Instant,
    ) -> Result<(Client, &TargetBinding, Credential), AccessOutcome> {
        let binding = self
            .targets
            .get(&id)
            .ok_or(Check::ServiceAccount.unknown())?;
        if !dns_label(&binding.watched_namespace) {
            return Err(Check::ServiceAccount.unknown());
        }
        let resolved = self.resolve(binding.context_id, deadline).await?;
        if super::super::destination::fingerprint(&resolved.config)? != binding.cluster_identity {
            return Err(Check::ServiceAccount.unknown());
        }
        let credential = resolved.credential;
        let creator =
            Client::try_from(resolved.config).map_err(|_| Check::ServiceAccount.unknown())?;
        self.check_binding(binding, &creator, deadline).await?;
        Ok((creator, binding, credential))
    }

    pub async fn service_accounts(
        &self,
        id: TargetId,
        deadline: Instant,
    ) -> Result<ServiceAccountsView, ClusterFailure> {
        within(deadline, Check::ServiceAccount, async {
            let (creator, binding, credential) = self.account_creator(id, deadline).await?;
            let bearer = match &credential {
                Credential::Bearer { value, .. } => value.as_str(),
                Credential::CertificateOnly => "",
            };
            let request = Request::new(format!(
                "/api/v1/namespaces/{}/serviceaccounts",
                binding.watched_namespace
            ));
            let mut names = Vec::new();
            let mut seen_names = HashSet::new();
            let mut seen_cursors = HashSet::new();
            let mut after = None;
            for _ in 0..5 {
                let mut params = ListParams::default().limit(200);
                params.continue_token = after;
                let request = request
                    .list(&params)
                    .map_err(|_| Check::ServiceAccount.unknown())?;
                let page = bounded(&creator, request, deadline).await?;
                let entries = items(&page)?;
                // An API ignoring limit must not defeat the total-name bound.
                if entries.len() > 200 {
                    return Err(Check::ServiceAccount.unknown());
                }
                for item in entries {
                    let name = item["metadata"]["name"]
                        .as_str()
                        .ok_or(Check::ServiceAccount.unknown())?;
                    if !dns_subdomain(name)
                        || echoed(&item["metadata"]["name"], bearer)
                        || item["metadata"]["namespace"].as_str()
                            != Some(&binding.watched_namespace)
                        || !seen_names.insert(name.to_owned())
                    {
                        return Err(Check::ServiceAccount.unknown());
                    }
                    names.push(name.to_owned());
                }
                after = continuation(&page)?;
                let Some(cursor) = after.as_ref() else {
                    return Ok(ServiceAccountsView {
                        names,
                        truncated: false,
                    });
                };
                if !seen_cursors.insert(cursor.clone()) {
                    return Err(Check::ServiceAccount.unknown());
                }
            }
            Ok(ServiceAccountsView {
                names,
                truncated: true,
            })
        })
        .await
        .map_err(failure)
    }

    pub async fn service_account_preview(
        &self,
        id: TargetId,
        name: &str,
        deadline: Instant,
    ) -> Result<ServiceAccountPreview, ClusterFailure> {
        within(deadline, Check::ServiceAccount, async {
            if !dns_subdomain(name) {
                return Err(Check::ServiceAccount.unknown());
            }
            let (creator, binding, _) = self.account_creator(id, deadline).await?;
            let namespace = &binding.watched_namespace;
            let request = Request::new(format!(
                "/apis/authorization.k8s.io/v1/namespaces/{namespace}/localsubjectaccessreviews"
            ));
            let mut preview = ServiceAccountPreview {
                status: PermissionStatus::Allowed,
                checks: ORKA_PERMISSIONS.iter().map(|(resource, verb)| PermissionCheck {
                    resource, verb, status: PermissionStatus::NotChecked,
                }).collect(),
            };
            for check in &mut preview.checks {
                let review = serde_json::json!({
                    "apiVersion":"authorization.k8s.io/v1", "kind":"LocalSubjectAccessReview",
                    "spec":{
                        "user":format!("system:serviceaccount:{namespace}:{name}"),
                        "groups":["system:serviceaccounts",format!("system:serviceaccounts:{namespace}"),"system:authenticated"],
                        "resourceAttributes":{"namespace":namespace,"group":"core.orka.ai","resource":check.resource,"verb":check.verb}
                    }
                });
                let result = async {
                    let body = serde_json::to_vec(&review).map_err(|_| Check::ServiceAccount.unknown())?;
                    let request = request.create(&PostParams::default(), body).map_err(|_| Check::ServiceAccount.unknown())?;
                    bounded(&creator, request, deadline).await
                }.await;
                // API denial is not an evaluated candidate denial. Never retry/fall back.
                check.status = result.as_ref().map(evaluated).unwrap_or(PermissionStatus::Unknown);
                if check.status != PermissionStatus::Allowed {
                    preview.status = check.status;
                    break;
                }
            }
            Ok(preview)
        }).await.map_err(failure)
    }
}

fn evaluated(review: &Value) -> PermissionStatus {
    let status = &review["status"];
    let allowed = status["allowed"].as_bool();
    let denied = match status.get("denied") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        _ => return PermissionStatus::Unknown,
    };
    match status.get("evaluationError") {
        None | Some(Value::Null) => (),
        Some(Value::String(value)) if value.is_empty() => (),
        _ => return PermissionStatus::Unknown,
    }
    match allowed {
        Some(true) if !denied => PermissionStatus::Allowed,
        Some(false) => PermissionStatus::Denied,
        _ => PermissionStatus::Unknown,
    }
}

/// The public raw transport avoids kube's eager JSON/error-body decoding.
async fn bounded(
    client: &Client,
    request: http::Request<Vec<u8>>,
    deadline: Instant,
) -> Result<Value, AccessOutcome> {
    within(
        deadline.min(Instant::now() + REQUEST_TIMEOUT),
        Check::ServiceAccount,
        async {
            let response = client
                .send(request.map(Body::from))
                .await
                .map_err(|e| kube_error(Check::ServiceAccount, e))?;
            if let Some(length) = response.headers().get(http::header::CONTENT_LENGTH) {
                let length = length
                    .to_str()
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or(Check::ServiceAccount.unknown())?;
                if length > CAP as u64 {
                    return Err(Check::ServiceAccount.unknown());
                }
            }
            if !response.status().is_success() {
                return Err(
                    Check::ServiceAccount.fail(match response.status().as_u16() {
                        401 => CheckResult::AuthenticationRejected,
                        403 => CheckResult::PermissionDenied,
                        _ => CheckResult::Unknown,
                    }),
                );
            }
            let mut body = response.into_body();
            let mut bytes = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|_| Check::ServiceAccount.unknown())?;
                if let Some(data) = frame.data_ref() {
                    if data.len() > CAP - bytes.len() {
                        return Err(Check::ServiceAccount.unknown());
                    }
                    bytes.extend_from_slice(data);
                }
            }
            serde_json::from_slice(&bytes).map_err(|_| Check::ServiceAccount.unknown())
        },
    )
    .await
}
