use super::*;
use serde_json::{json, Value};

pub(super) const MANAGER: &str = "bridge-copilot";
pub(super) const ENVIRONMENT: &str = "bridge.orka.ai/environment";
pub(super) const WORKSPACE: &str = "bridge.orka.ai/workspace";

pub(super) fn manifests(r: &Receipt) -> Result<[Value; 3], Error> {
    r.validate()?;
    let labels = json!({"app.kubernetes.io/managed-by": MANAGER,
        ENVIRONMENT: r.environment_id.to_string(), WORKSPACE: r.workspace_id.to_string()});
    let metadata = json!({"name": r.resource_name(), "namespace": r.namespace, "labels": labels});
    let selector = json!({ENVIRONMENT: r.environment_id.to_string()});
    Ok([
        json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":metadata,
            "spec":{"accessModes":["ReadWriteOnce"],"resources":{"requests":{"storage":"10Gi"}}}}),
        json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":metadata,
            "spec":{"podSelector":{"matchLabels":selector},"policyTypes":["Ingress","Egress"],"ingress":[],
                "egress":[{"ports":[{"protocol":"UDP","port":53},{"protocol":"TCP","port":53}]},
                    {"to":[{"ipBlock":{"cidr":"0.0.0.0/0","except":["0.0.0.0/8","10.0.0.0/8","100.64.0.0/10","127.0.0.0/8","169.254.0.0/16","172.16.0.0/12","192.168.0.0/16","224.0.0.0/4","240.0.0.0/4"]}},
                        {"ipBlock":{"cidr":"::/0","except":["::/128","::1/128","fc00::/7","fe80::/10","ff00::/8"]}}],
                        "ports":[{"protocol":"TCP","port":443}]}]}}),
        json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":metadata,"spec":{
            // Record the Deployment UID before Start can create a credential-bearing pod.
            "replicas":0,"strategy":{"type":"Recreate"},"selector":{"matchLabels":selector},
            "template":{"metadata":{"labels":labels},"spec":{
                "automountServiceAccountToken":false,"enableServiceLinks":false,
                "hostNetwork":false,"hostPID":false,"hostIPC":false,
                "securityContext":{"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,"fsGroup":1000,
                    "seccompProfile":{"type":"RuntimeDefault"}},
                "containers":[{"name":"environment","image":r.image,
                    "command":["/usr/local/bin/bridge-copilot-environment"],"args":["serve"],
                    "securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,
                        "runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,"privileged":false,
                        "procMount":"Default","seccompProfile":{"type":"RuntimeDefault"},
                        "capabilities":{"drop":["ALL"]}},
                    "resources":{"requests":{"cpu":"250m","memory":"512Mi"},"limits":{"cpu":"2","memory":"2Gi"}},
                    "volumeMounts":[{"name":"home","mountPath":"/home/creator"},{"name":"scratch","mountPath":"/tmp"}],
                    "livenessProbe":{"exec":{"command":["/usr/local/bin/bridge-copilot-environment","health"]},"periodSeconds":10,"failureThreshold":3},
                    "readinessProbe":{"exec":{"command":["/usr/local/bin/bridge-copilot-environment","ready"]},"periodSeconds":5}}],
                "volumes":[{"name":"home","persistentVolumeClaim":{"claimName":r.resource_name()}},
                    {"name":"scratch","emptyDir":{"medium":"Memory","sizeLimit":"128Mi"}}]}}}}),
    ])
}

/// Admission may add defaults, but it must not change any field this owner relies on.
pub(super) fn contains(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => e
            .iter()
            .all(|(k, v)| a.get(k).is_some_and(|x| contains(x, v))),
        (Value::Array(a), Value::Array(e)) => {
            a.len() == e.len() && a.iter().zip(e).all(|(a, e)| contains(a, e))
        }
        _ => actual == expected,
    }
}
