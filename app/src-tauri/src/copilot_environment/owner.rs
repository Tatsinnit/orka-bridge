use super::*;
use k8s_openapi::api::authorization::v1::SelfSubjectAccessReview;
use kube::{
    api::{DeleteParams, DynamicObject, ListParams, Patch, PatchParams, PostParams, Preconditions},
    core::{ApiResource, GroupVersionKind},
    Api, Client,
};
use serde_json::json;
use std::{collections::BTreeMap, future::Future, path::PathBuf, time::Duration};

pub struct Connection {
    pub(super) target: Target,
    pub(super) client: Client,
}
impl Connection {
    /// Call with freshly resolved native credentials, not a Send RemoteAccess or renderer URL.
    pub fn new(context: &str, config: Config) -> Result<Self, Error> {
        let target = Target::from_config(context, &config)?;
        let client = Client::try_from(config).map_err(|_| Error::Unavailable)?;
        Ok(Self { target, client })
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct EnvironmentView {
    pub name: String,
    pub namespace: String,
    pub environment_id: String,
    pub deployment_uid: Option<String>,
    pub pvc_uid: Option<String>,
    pub policy_uid: Option<String>,
    pub managed: bool,
}
pub struct Owner {
    root: PathBuf,
}
impl Owner {
    pub fn new(app_data: PathBuf) -> Self {
        Self { root: app_data }
    }
    pub fn receipts(&self) -> Result<Vec<Receipt>, Error> {
        store::Store::new(&self.root)?.all()
    }
    pub async fn start(&self, c: &Connection, id: Uuid) -> Result<(), Error> {
        let store = store::Store::new(&self.root)?;
        let r = bound(&store, c, id)?;
        for i in 0..3 {
            let object = get_owned(c, &r, i).await?.ok_or(Error::Conflict)?;
            if object.metadata.deletion_timestamp.is_some() {
                return Err(Error::Pending);
            }
        }
        scale(c, &r, 1).await
    }
    pub async fn stop(&self, c: &Connection, id: Uuid) -> Result<(), Error> {
        let store = store::Store::new(&self.root)?;
        let r = bound(&store, c, id)?;
        stop(c, &r).await
    }
    pub async fn remove(&self, c: &Connection, id: Uuid) -> Result<(), Error> {
        let store = store::Store::new(&self.root)?;
        let mut r = bound(&store, c, id)?;
        // Check all candidates before the first destructive action, including partial setup.
        for i in 0..3 {
            get_owned(c, &r, i).await?;
        }
        stop(c, &r).await?;
        for i in [2, 1, 0] {
            if let Some(object) = get_owned(c, &r, i).await? {
                if i == 2 && object.data["spec"]["replicas"] != 0 {
                    return Err(Error::Conflict);
                }
                let params = DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: object.metadata.uid,
                        resource_version: object.metadata.resource_version,
                    }),
                    ..DeleteParams::default()
                };
                request(api(c, &r.namespace, i).delete(&r.resource_name(), &params)).await?;
                if request(api(c, &r.namespace, i).get_opt(&r.resource_name()))
                    .await?
                    .is_some()
                {
                    return Err(Error::Pending);
                }
            }
            // Background garbage collection may leave live children after Deployment deletion.
            if i == 2 {
                stop(c, &r).await?;
            }
            *uid_mut(&mut r, i) = None;
            store.save(&r)?;
        }
        store.remove(id)
    }
    pub async fn discover(
        &self,
        c: &Connection,
        namespace: &str,
    ) -> Result<Vec<EnvironmentView>, Error> {
        // Validate namespace before constructing namespaced API paths.
        if !valid_namespace(namespace) {
            return Err(Error::Invalid);
        }
        let store = store::Store::new(&self.root)?;
        let receipts = store.all()?;
        let mut found: BTreeMap<String, [Option<DynamicObject>; 3]> = BTreeMap::new();
        let params = ListParams::default().labels(&format!(
            "app.kubernetes.io/managed-by={}",
            resources::MANAGER
        ));
        for i in 0..3 {
            let objects = request(api(c, namespace, i).list(&params)).await?;
            for object in objects {
                let Some(labels) = object.metadata.labels.as_ref() else {
                    continue;
                };
                if labels
                    .get("app.kubernetes.io/managed-by")
                    .map(String::as_str)
                    != Some(resources::MANAGER)
                    || !labels.contains_key(resources::ENVIRONMENT)
                {
                    continue;
                }
                if let Some(name) = object.metadata.name.clone() {
                    found.entry(name).or_default()[i] = Some(object);
                }
            }
        }
        Ok(found
            .into_iter()
            .map(|(name, objects)| {
                let managed = receipts.iter().any(|r| {
                    r.target == c.target
                        && r.namespace == namespace
                        && r.resource_name() == name
                        && (0..3).all(|i| match &objects[i] {
                            Some(o) => verify(r, i, o).is_ok(),
                            None => uid(r, i).is_none(),
                        })
                });
                let environment_id = objects
                    .iter()
                    .flatten()
                    .find_map(|o| {
                        o.metadata
                            .labels
                            .as_ref()?
                            .get(resources::ENVIRONMENT)
                            .cloned()
                    })
                    .unwrap_or_default();
                EnvironmentView {
                    name,
                    namespace: namespace.into(),
                    environment_id,
                    managed,
                    pvc_uid: objects[0].as_ref().and_then(|o| o.metadata.uid.clone()),
                    policy_uid: objects[1].as_ref().and_then(|o| o.metadata.uid.clone()),
                    deployment_uid: objects[2].as_ref().and_then(|o| o.metadata.uid.clone()),
                }
            })
            .collect())
    }
    /// Provision stopped resources. Only explicit Start may create a pod, after UID persistence.
    pub async fn create(
        &self,
        c: &Connection,
        workspace: Uuid,
        namespace: &str,
        image: &str,
    ) -> Result<Receipt, Error> {
        let mut r = Receipt {
            version: 1,
            environment_id: Uuid::new_v4(),
            workspace_id: workspace,
            target: c.target.clone(),
            namespace: namespace.into(),
            image: image.into(),
            deployment_uid: None,
            pvc_uid: None,
            policy_uid: None,
        };
        r.validate()?;
        let store = store::Store::new(&self.root)?;
        if let Some(old) = store
            .all()?
            .into_iter()
            .find(|r| r.workspace_id == workspace)
        {
            if old.target != c.target {
                return Err(Error::TargetChanged);
            }
            if old.namespace != namespace || old.image != image {
                return Err(Error::Conflict);
            }
            r = old;
        }
        permissions(c, namespace).await?;
        store.save(&r)?;
        for (i, expected) in resources::manifests(&r)?.into_iter().enumerate() {
            let api = api(c, namespace, i);
            let found = request(api.get_opt(&r.resource_name())).await?;
            if let Some(found) = found {
                verify(&r, i, &found)?;
                if found.metadata.deletion_timestamp.is_some() {
                    return Err(Error::Pending);
                }
            } else {
                if uid(&r, i).is_some() {
                    return Err(Error::Conflict);
                }
                let expected: DynamicObject =
                    serde_json::from_value(expected).map_err(|_| Error::Invalid)?;
                let made = request(api.create(&PostParams::default(), &expected)).await?;
                let id = made
                    .metadata
                    .uid
                    .filter(|s| !s.is_empty())
                    .ok_or(Error::Unavailable)?;
                *uid_mut(&mut r, i) = Some(id);
                store.save(&r)?;
            }
        }
        Ok(r)
    }
}

fn bound(store: &store::Store, c: &Connection, id: Uuid) -> Result<Receipt, Error> {
    let r = store.load(id)?;
    if r.target != c.target {
        return Err(Error::TargetChanged);
    }
    Ok(r)
}
async fn get_owned(c: &Connection, r: &Receipt, i: usize) -> Result<Option<DynamicObject>, Error> {
    let found = request(api(c, &r.namespace, i).get_opt(&r.resource_name())).await?;
    if let Some(object) = &found {
        verify(r, i, object)?;
    }
    Ok(found)
}
async fn scale(c: &Connection, r: &Receipt, replicas: u32) -> Result<(), Error> {
    let object = get_owned(c, r, 2).await?.ok_or(Error::Conflict)?;
    if object.metadata.deletion_timestamp.is_some() {
        return Err(Error::Pending);
    }
    let version = object
        .metadata
        .resource_version
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or(Error::Unavailable)?;
    // A no-op replica patch may not advance resourceVersion. Force a write so a timed-out,
    // delayed Start with the previous version cannot commit after a successful Stop.
    let body = json!({"metadata":{"uid":r.deployment_uid,"resourceVersion":version,
        "annotations":{"bridge.orka.ai/control":Uuid::new_v4().to_string()}},"spec":{"replicas":replicas}});
    request(api(c, &r.namespace, 2).patch(
        &r.resource_name(),
        &PatchParams::default(),
        &Patch::Merge(body),
    ))
    .await?;
    Ok(())
}
async fn stop(c: &Connection, r: &Receipt) -> Result<(), Error> {
    let operation = async {
        if get_owned(c, r, 2).await?.is_some() {
            scale(c, r, 0).await?;
        }
        loop {
            let deployment = get_owned(c, r, 2).await?;
            if deployment
                .as_ref()
                .is_some_and(|d| d.data["spec"]["replicas"] != 0)
            {
                return Err(Error::Conflict);
            }
            let mut active = deployment.as_ref().is_some_and(|d| !observed_zero(d));
            // An empty Pod list can precede a controller's pending create. Keep policy/storage
            // until scale-down is observed, and after parent deletion until children are gone.
            let mut sets = BTreeMap::new();
            for rs in request(api(c, &r.namespace, 4).list(&ListParams::default())).await? {
                let owned = rs.metadata.owner_references.as_ref().is_some_and(|refs| {
                    refs.iter().any(|reference| {
                        reference.controller == Some(true)
                            && reference.kind == "Deployment"
                            && reference.api_version == "apps/v1"
                            && reference.name == r.resource_name()
                            && Some(&reference.uid) == r.deployment_uid.as_ref()
                    })
                });
                if owned || may_use_environment(&rs, r) {
                    active |= deployment.is_none() || !observed_zero(&rs);
                }
                if let (Some(name), Some(uid)) = (rs.metadata.name.clone(), rs.metadata.uid.clone())
                {
                    sets.insert((name, uid), Some(rs));
                }
            }
            let pods = request(api(c, &r.namespace, 3).list(&ListParams::default())).await?;
            for pod in pods {
                if conclusively_terminated(&pod) {
                    continue;
                }
                // Hints may block cleanup, never authorize mutation. Orphaning must not turn a live
                // coding pod into proof of Stop, or remove its policy while its PVC is still mounted.
                if may_use_environment(&pod, r) {
                    active = true;
                    continue;
                }
                let Some(reference) = pod.metadata.owner_references.as_ref().and_then(|refs| {
                    refs.iter().find(|r| {
                        r.controller == Some(true)
                            && r.kind == "ReplicaSet"
                            && r.api_version == "apps/v1"
                    })
                }) else {
                    continue;
                };
                let key = (reference.name.clone(), reference.uid.clone());
                if let std::collections::btree_map::Entry::Vacant(entry) = sets.entry(key.clone()) {
                    entry.insert(request(api(c, &r.namespace, 4).get_opt(&reference.name)).await?);
                }
                let Some(rs) = sets.get(&key).and_then(Option::as_ref) else {
                    continue;
                };
                if rs.metadata.uid.as_deref() != Some(&reference.uid) {
                    continue;
                }
                if rs.metadata.owner_references.as_ref().is_some_and(|refs| {
                    refs.iter().any(|reference| {
                        reference.controller == Some(true)
                            && reference.kind == "Deployment"
                            && reference.api_version == "apps/v1"
                            && reference.name == r.resource_name()
                            && Some(&reference.uid) == r.deployment_uid.as_ref()
                    })
                }) {
                    active = true;
                }
            }
            if !active {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(20), operation)
        .await
        .map_err(|_| Error::Pending)?
}

fn observed_zero(object: &DynamicObject) -> bool {
    object.metadata.generation.is_some_and(|generation| {
        object.data["status"]["observedGeneration"]
            .as_i64()
            .is_some_and(|seen| seen >= generation)
    }) && object.data["spec"]["replicas"] == 0
        && object.data["status"]["replicas"].as_i64().unwrap_or(0) == 0
}
fn may_use_environment(object: &DynamicObject, r: &Receipt) -> bool {
    object.metadata.labels.as_ref().is_some_and(|labels| {
        labels.get(resources::ENVIRONMENT) == Some(&r.environment_id.to_string())
    }) || [
        &object.data["spec"]["volumes"],
        &object.data["spec"]["template"]["spec"]["volumes"],
    ]
    .iter()
    .any(|volumes| {
        volumes.as_array().is_some_and(|volumes| {
            volumes
                .iter()
                .any(|v| v["persistentVolumeClaim"]["claimName"] == r.resource_name())
        })
    })
}
fn conclusively_terminated(pod: &DynamicObject) -> bool {
    let status = &pod.data["status"];
    if !matches!(status["phase"].as_str(), Some("Failed" | "Succeeded")) {
        return false;
    }
    for (spec, state) in [
        ("containers", "containerStatuses"),
        ("initContainers", "initContainerStatuses"),
        ("ephemeralContainers", "ephemeralContainerStatuses"),
    ] {
        let expected = pod.data["spec"][spec]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let reported = status[state]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        if (spec == "containers" && expected.is_empty())
            || expected.len() != reported.len()
            || !expected.iter().all(|container| {
                reported.iter().any(|reported| {
                    reported["name"] == container["name"]
                        && reported["state"]["terminated"].is_object()
                        && reported["state"]["running"].is_null()
                        && reported["state"]["waiting"].is_null()
                })
            })
        {
            return false;
        }
    }
    true
}

fn api(c: &Connection, ns: &str, i: usize) -> Api<DynamicObject> {
    let (group, version, kind, plural) = match i {
        0 => ("", "v1", "PersistentVolumeClaim", "persistentvolumeclaims"),
        1 => (
            "networking.k8s.io",
            "v1",
            "NetworkPolicy",
            "networkpolicies",
        ),
        2 => ("apps", "v1", "Deployment", "deployments"),
        3 => ("", "v1", "Pod", "pods"),
        _ => ("apps", "v1", "ReplicaSet", "replicasets"),
    };
    let mut resource = ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, kind));
    resource.plural = plural.into();
    Api::namespaced_with(c.client.clone(), ns, &resource)
}
fn uid(r: &Receipt, i: usize) -> &Option<String> {
    match i {
        0 => &r.pvc_uid,
        1 => &r.policy_uid,
        _ => &r.deployment_uid,
    }
}
fn uid_mut(r: &mut Receipt, i: usize) -> &mut Option<String> {
    match i {
        0 => &mut r.pvc_uid,
        1 => &mut r.policy_uid,
        _ => &mut r.deployment_uid,
    }
}
fn verify(r: &Receipt, i: usize, object: &DynamicObject) -> Result<(), Error> {
    if uid(r, i).is_none()
        || object.metadata.uid.as_ref() != uid(r, i).as_ref()
        || object.metadata.name.as_deref() != Some(&r.resource_name())
        || object.metadata.namespace.as_deref() != Some(&r.namespace)
    {
        return Err(Error::Conflict);
    }
    let mut actual = serde_json::to_value(object).map_err(|_| Error::Unavailable)?;
    // Go's API encoding omits these zero-valued fields. Missing has exactly the same policy.
    if i == 1 {
        actual["spec"]
            .as_object_mut()
            .ok_or(Error::Conflict)?
            .entry("ingress")
            .or_insert_with(|| json!([]));
    }
    if i == 2 {
        let pod = actual["spec"]["template"]["spec"]
            .as_object_mut()
            .ok_or(Error::Conflict)?;
        for key in ["hostNetwork", "hostPID", "hostIPC"] {
            pod.entry(key).or_insert_with(|| json!(false));
        }
    }
    let mut expected = resources::manifests(r)?[i].clone();
    if i == 1 {
        let selector = &mut actual["spec"]["podSelector"];
        if !(selector["matchExpressions"].is_null()
            || selector["matchExpressions"]
                .as_array()
                .is_some_and(Vec::is_empty))
        {
            return Err(Error::Conflict);
        }
        selector
            .as_object_mut()
            .ok_or(Error::Conflict)?
            .remove("matchExpressions");
        // Added selector constraints can select no Pods; added port end-ranges can broaden egress.
        if actual["spec"] != expected["spec"] {
            return Err(Error::Conflict);
        }
    }
    if i == 2 {
        if actual["spec"]["template"]["metadata"]["annotations"]
            .as_object()
            .is_some_and(|annotations| {
                annotations.iter().any(|(key, value)| {
                    key.starts_with("container.apparmor.security.beta.kubernetes.io/")
                        && value != "runtime/default"
                })
            })
        {
            return Err(Error::Conflict);
        }
        let pod = &actual["spec"]["template"]["spec"];
        let container = &pod["containers"][0];
        for security in [&pod["securityContext"], &container["securityContext"]] {
            let profile = &security["appArmorProfile"];
            let selinux = &security["seLinuxOptions"];
            if !(profile.is_null() || *profile == json!({"type":"RuntimeDefault"}))
                || !(selinux.is_null()
                    || selinux.as_object().is_some_and(serde_json::Map::is_empty))
            {
                return Err(Error::Conflict);
            }
        }
        for key in ["lifecycle", "startupProbe"] {
            if !(container[key].is_null()
                || container[key]
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty))
            {
                return Err(Error::Conflict);
            }
        }
        // The pinned image owns startup defaults; extra environment entries can replace them.
        for key in ["env", "envFrom"] {
            if !(container[key].is_null() || container[key].as_array().is_some_and(Vec::is_empty)) {
                return Err(Error::Conflict);
            }
        }
        let added = &actual["spec"]["template"]["spec"]["containers"][0]["securityContext"]
            ["capabilities"]["add"];
        if !(added.is_null() || added.as_array().is_some_and(Vec::is_empty)) {
            return Err(Error::Conflict);
        }
        expected["spec"]
            .as_object_mut()
            .ok_or(Error::Invalid)?
            .remove("replicas");
        if actual["spec"]["template"]["spec"]["initContainers"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
        {
            return Err(Error::Conflict);
        }
    }
    if !resources::contains(&actual, &expected) {
        return Err(Error::Conflict);
    }
    Ok(())
}
async fn request<T>(future: impl Future<Output = Result<T, kube::Error>>) -> Result<T, Error> {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|e| match e {
            kube::Error::Api(e) if matches!(e.code, 401 | 403) => Error::Denied,
            kube::Error::Api(e) if matches!(e.code, 409 | 422) => Error::Conflict,
            _ => Error::Unavailable,
        })
}
async fn permissions(c: &Connection, ns: &str) -> Result<(), Error> {
    let api: Api<SelfSubjectAccessReview> = Api::all(c.client.clone());
    for (group, resource, verbs) in [
        ("", "persistentvolumeclaims", "get list create delete"),
        (
            "networking.k8s.io",
            "networkpolicies",
            "get list create delete",
        ),
        ("apps", "deployments", "get list create patch delete"),
        ("apps", "replicasets", "get list"),
        ("", "pods", "get list"),
    ] {
        for verb in verbs.split_whitespace() {
            let review = serde_json::from_value(json!({"spec":{"resourceAttributes":{
                "namespace":ns,"group":group,"resource":resource,"verb":verb}}}))
            .map_err(|_| Error::Invalid)?;
            let result = request(api.create(&PostParams::default(), &review)).await?;
            if !result.status.is_some_and(|s| s.allowed) {
                return Err(Error::Denied);
            }
        }
    }
    Ok(())
}
