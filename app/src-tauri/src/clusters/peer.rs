use super::{kube_error, ErrorKind, ForwardPeer, TargetBinding};
use k8s_openapi::{
    api::{
        apps::v1::{Deployment, ReplicaSet},
        core::v1::{Pod, Service, ServicePort},
    },
    apimachinery::{pkg::apis::meta::v1::OwnerReference, pkg::util::intstr::IntOrString},
    NamespaceResourceScope,
};
use kube::{api::ListParams, Api, Client, Resource, ResourceExt};
use std::collections::BTreeMap;

pub(super) struct SelectedPeer {
    pub peer: ForwardPeer,
    controller: OwnerReference,
    selector: BTreeMap<String, String>,
    service_port: ServicePort,
}

// Keep the existing list authority, scoped to the named object. A partial or
// ambiguous reply is not evidence that the frozen identity still exists.
async fn exact<T>(creator: &Client, namespace: &str, name: &str, uid: &str) -> Result<T, ErrorKind>
where
    T: Resource<DynamicType = (), Scope = NamespaceResourceScope>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
{
    let mut list = Api::<T>::namespaced(creator.clone(), namespace)
        .list(
            &ListParams::default()
                .fields(&format!("metadata.name={name}"))
                .limit(2),
        )
        .await
        .map_err(kube_error)?;
    if list
        .metadata
        .continue_
        .as_ref()
        .is_some_and(|c| !c.is_empty())
        || list.items.len() != 1
    {
        return Err(ErrorKind::Identity);
    }
    let object = list.items.pop().unwrap();
    if object.meta().name.as_deref() != Some(name)
        || object.namespace().as_deref() != Some(namespace)
        || object.uid().as_deref() != Some(uid)
    {
        return Err(ErrorKind::Identity);
    }
    Ok(object)
}
fn controller(owners: &[OwnerReference], kind: &str) -> Option<OwnerReference> {
    let mut controllers = owners.iter().filter(|o| o.controller == Some(true));
    let owner = controllers.next()?;
    (controllers.next().is_none()
        && owner.api_version == "apps/v1"
        && owner.kind == kind
        && !owner.name.is_empty()
        && !owner.uid.is_empty())
    .then(|| owner.clone())
}
async fn service(
    creator: &Client,
    binding: &TargetBinding,
) -> Result<(BTreeMap<String, String>, ServicePort), ErrorKind> {
    let service: Service = exact(
        creator,
        &binding.service_namespace,
        &binding.service_name,
        &binding.service_uid,
    )
    .await?;
    let spec = service.spec.ok_or(ErrorKind::Identity)?;
    let selector = spec
        .selector
        .filter(|s| !s.is_empty())
        .ok_or(ErrorKind::Identity)?;
    let mut ports = spec
        .ports
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.name.as_deref() == Some("api"));
    let port = ports.next().ok_or(ErrorKind::Identity)?;
    if ports.next().is_some() || !port.protocol.as_deref().is_none_or(|p| p == "TCP") {
        return Err(ErrorKind::Identity);
    }
    Ok((selector, port))
}
async fn deployment(creator: &Client, binding: &TargetBinding) -> Result<(), ErrorKind> {
    // Owner references cannot cross namespaces.
    if binding.service_namespace != binding.deployment_namespace {
        return Err(ErrorKind::Identity);
    }
    let deployment: Deployment = exact(
        creator,
        &binding.deployment_namespace,
        &binding.deployment_name,
        &binding.deployment_uid,
    )
    .await?;
    if deployment.metadata.deletion_timestamp.is_some()
        || !super::super::client::deployment_watches(&deployment, &binding.watched_namespace)
    {
        return Err(ErrorKind::Identity);
    }
    Ok(())
}
fn ready(pod: &Pod) -> bool {
    pod.status.as_ref().is_some_and(|s| {
        s.phase.as_deref() == Some("Running")
            && s.conditions
                .as_ref()
                .is_some_and(|c| c.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
    })
}
fn pod_peer(
    pod: &Pod,
    binding: &TargetBinding,
    selector: &BTreeMap<String, String>,
    port: &ServicePort,
) -> Option<(ForwardPeer, OwnerReference)> {
    if pod.namespace().as_deref() != Some(&binding.service_namespace)
        || pod.metadata.deletion_timestamp.is_some()
        || !selector
            .iter()
            .all(|(k, v)| pod.metadata.labels.as_ref().and_then(|l| l.get(k)) == Some(v))
        || !ready(pod)
    {
        return None;
    }
    let owner = controller(
        pod.metadata.owner_references.as_deref().unwrap_or_default(),
        "ReplicaSet",
    )?;
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
    }?;
    Some((
        ForwardPeer {
            pod_name: pod.metadata.name.clone().filter(|s| !s.is_empty())?,
            pod_namespace: binding.service_namespace.clone(),
            pod_uid: pod.uid().filter(|s| !s.is_empty())?,
            port: u16::try_from(number).ok().filter(|p| *p > 0)?,
        },
        owner,
    ))
}
async fn owner_chain(
    creator: &Client,
    binding: &TargetBinding,
    owner: &OwnerReference,
) -> Result<(), ErrorKind> {
    let rs: ReplicaSet =
        exact(creator, &binding.service_namespace, &owner.name, &owner.uid).await?;
    let deployment = controller(
        rs.metadata.owner_references.as_deref().unwrap_or_default(),
        "Deployment",
    )
    .ok_or(ErrorKind::Identity)?;
    if rs.metadata.deletion_timestamp.is_some()
        || deployment.name != binding.deployment_name
        || deployment.uid != binding.deployment_uid
    {
        return Err(ErrorKind::Identity);
    }
    Ok(())
}
pub(super) async fn select(
    creator: &Client,
    binding: &TargetBinding,
) -> Result<SelectedPeer, ErrorKind> {
    let (selector, service_port) = service(creator, binding).await?;
    deployment(creator, binding).await?;
    let labels = selector
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    let pods = Api::<Pod>::namespaced(creator.clone(), &binding.service_namespace)
        .list(&ListParams::default().labels(&labels))
        .await
        .map_err(kube_error)?;
    let has_ready_pod = pods
        .iter()
        .any(|pod| pod.metadata.deletion_timestamp.is_none() && ready(pod));
    for pod in pods {
        let Some((peer, controller)) = pod_peer(&pod, binding, &selector, &service_port) else {
            continue;
        };
        match owner_chain(creator, binding, &controller).await {
            Ok(()) => {
                return Ok(SelectedPeer {
                    peer,
                    controller,
                    selector,
                    service_port,
                })
            }
            // A foreign candidate must not hide a valid rollout Pod. Kubernetes
            // authentication/transport failures still stop preparation, not retry.
            Err(ErrorKind::Identity) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(if has_ready_pod {
        ErrorKind::Identity
    } else {
        ErrorKind::Transport
    })
}
pub(super) async fn validate(
    creator: &Client,
    binding: &TargetBinding,
    selected: &SelectedPeer,
) -> Result<(), ErrorKind> {
    let (selector, service_port) = service(creator, binding).await?;
    if selector != selected.selector || service_port != selected.service_port {
        return Err(ErrorKind::Identity);
    }
    deployment(creator, binding).await?;
    let pod: Pod = exact(
        creator,
        &selected.peer.pod_namespace,
        &selected.peer.pod_name,
        &selected.peer.pod_uid,
    )
    .await?;
    let (peer, controller) =
        pod_peer(&pod, binding, &selector, &service_port).ok_or(ErrorKind::Identity)?;
    if peer != selected.peer || controller != selected.controller {
        return Err(ErrorKind::Identity);
    }
    owner_chain(creator, binding, &controller).await
}
