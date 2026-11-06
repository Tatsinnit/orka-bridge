# Shared Orka clusters

**Share the cluster, not the Orka installation.** For example, team A and team B
can use one Kubernetes cluster while each keeps its own Orka release, watched
namespace, runtime namespace, API endpoint and credentials. Team B cannot manage
team A's Agents, Tasks or Sessions; an observer needs an explicit read grant and
a namespace-local identity for each installation it observes.

This is a proposed access model with Kubernetes authorization validation.
Bridge does not configure these namespaces or roles today and has no credential
store or shared-cluster connection flow.

## Decision

A Kubernetes cluster can be shared, but each mutually untrusted creator or team
needs its own Orka trust domain:

- One watched namespace.
- One Orka release and API endpoint.
- One separate runtime namespace.
- One controller identity, database, key set, PVC set, and credential set.

Creators in the same watched namespace are one trust domain. Kubernetes RBAC
cannot express "may create arbitrary Agents, but may later read or change only
the objects I created." It has no label-, owner-, or creator-aware rules. Static
`resourceNames` rules do not solve dynamic creation, collection listing,
generated Task names, or Session identifiers.

The selected Orka revision reinforces this boundary. A controller requires one
non-empty `--watch-namespace`, requires namespace isolation, and scopes its
default cache to that namespace. The release namespace must match the watched
namespace. A single controller serving several creator namespaces would need
the work discussed in
[orka-agents/orka#630](https://github.com/orka-agents/orka/issues/630).
The current recommendation there is also to share cluster-level infrastructure
while keeping one controller and store per team namespace.

Multiple releases share cluster-scoped CRDs and admission resources. A platform
administrator owns those resources and applies compatible CRDs once. Tenant
releases use distinct names and namespaces and do not give their creators
control of cluster-scoped installation resources.

## Topology

```text
Kubernetes cluster
  platform administrator
    shared CRDs and admission policy

  creator-a namespace             creator-a-runtimes namespace
    Orka release A                  runtime workloads for release A
    Agents, Tasks, Sessions
    release A credentials

  creator-b namespace             creator-b-runtimes namespace
    Orka release B                  runtime workloads for release B
    Agents, Tasks, Sessions
    release B credentials
```

The runtime namespaces are controller-owned. Creators and observers receive no
direct access to their Pods, Jobs, ServiceAccounts, tokens, Secrets, or RBAC.

## Roles

The example rules live in
[`docs/shared-clusters/rbac.yaml`](shared-clusters/rbac.yaml). They are a design
fixture, not an installer. At the selected Orka revision, this creator/observer
model applies only to namespace-local ServiceAccounts authenticated through
Kubernetes TokenReview. Human identity-provider users are deferred to a separate
authentication and authorization design.

A human identity returned by TokenReview has no ServiceAccount namespace, so
Orka's namespace-isolated API refuses it even if Kubernetes RoleBindings permit
CRD access. Direct Orka OIDC authentication is a different path: it skips these
Kubernetes authorization checks. An OIDC caller with an observer RoleBinding
could therefore still create Tasks. These roles must not be presented as an
enforced read-only/creator boundary for direct OIDC users.

| Role | Scope | Access |
|---|---|---|
| Creator | One watched namespace | Manage Agents, Tasks, approvals and Sessions; read Providers, Tools and Skills |
| Observer | One or more explicitly enrolled installations | Read complete Agents, Tasks and Sessions |
| Platform administrator | Cluster and all Orka installations | Own namespaces, releases, CRDs, admission, RBAC, credentials and recovery |

Creator and observer access is defined by reusable ClusterRoles but granted
only through namespace-local RoleBindings. A ClusterRoleBinding would silently
extend access to current and future namespaces and is not part of this model.

### Creator

A creator may manage the agent execution records in one namespace:

- `agents`: `get`, `list`, `watch`, `create`, `update`, `patch`, `delete`.
- `tasks`: `get`, `list`, `watch`, `create`, `update`, `patch`, `delete`.
- `tasks/approvals`: `update`.
- Virtual `sessions`: `get`, `list`, `create`, `update`, `delete`.
- Virtual `chats`: `create`; `chats/config`: `get`.
- `providers`, `tools`, and `skills`: `get`, `list`, `watch`.

The role intentionally omits status writes and the explicit `/finalizers`
subresource. Those belong to the controller. Kubernetes still permits a caller
with top-level create, update or patch to change `metadata.finalizers`; RBAC
cannot make that field controller-only. If Orka cleanup relies on that
invariant, production clusters need a fail-closed admission policy covering
both top-level and `/finalizers` writes. The role also omits direct access to
Secrets, Pods, Jobs, Deployments, ServiceAccounts, token creation, Roles,
RoleBindings, `bind`, `escalate`, and impersonation.

### Observer

An observer can read Agents, Tasks and Sessions in each explicitly enrolled
installation. These permissions disclose complete records, not only the fields
Bridge happens to display. Agent reads include prompts and references. Task
reads through Orka include results, logs, traces, events, approvals and
artifacts. Session reads include transcripts, events and native Session export.
A Bridge UI projection is not an RBAC boundary.

This broad read role is intended for the issue's "read-only access to everyone's
agents" administrator or auditor. Ordinary creators do not receive it and keep
their Tasks and Sessions private from other creator identities.

Orka derives a TokenReview caller's namespace from a ServiceAccount identity.
A ServiceAccount from one namespace cannot use one token to read every
installation's virtual Session store, even if Kubernetes RoleBindings grant it
access elsewhere. The fixture therefore creates one observer ServiceAccount per
installation. Bridge would need a namespace-local observer identity and API
endpoint for each connected installation, then fan out those reads. Human IdP
users need a later design that enforces equivalent namespace and operation
checks on every supported API path before replacing these identities.

### Platform administrator

Installation is a platform operation. Bridge's selected source profile uses the
staged chart, `manifest_staging/charts/orka`, rather than the older checked-in
release chart. Its pinned
[`templates/rbac.yaml`](https://github.com/orka-agents/orka/blob/59703d6eac1760aa2dc5fed4dee9dcb74c6e5186/manifest_staging/charts/orka/templates/rbac.yaml)
binds controller Secret and Job access through a tenant RoleBinding, with a
separate runtime RoleBinding for runtime workloads and Secrets. An optional
agent-host namespace also has a separate binding. Each such namespace must be
installation-specific.

The staged chart also grants separate cluster authority for TokenReview,
SubjectAccessReview, selected shared resources and cross-namespace preflight
lists. A watched namespace limits reconciliation, not Kubernetes authority:
platform administrators must review the rendered roles and any optional
aggregated grants. The integration check should deny reads and writes of the
other installation's Secrets and workloads, while accounting for the explicitly
granted shared-resource and inventory reads.

The chart can create or use CRDs, admission resources, cluster and namespace
RBAC, workloads, Services,
PVCs, Secrets, NetworkPolicies and runtime resources. The disposable validation
cluster uses its built-in `cluster-admin` identity for this role rather than
pretending a static partial role is sufficient.

Controller and worker roles are not human roles. They include authority such as
Secret access, workload creation, ServiceAccount token creation and selected
RBAC operations. Bridge, creators and observers must never be bound to them.

## Version details and validation scope

The design was checked against:

- Bridge baseline `80fb6e4`.
- The Orka source revision selected by
  [`images/guest/versions.json`](../images/guest/versions.json),
  `59703d6eac1760aa2dc5fed4dee9dcb74c6e5186`, including its staged chart.
- kind v0.33.0 and the digest-pinned Kubernetes v1.35.8 node image used by
  [`scripts/validate_shared_cluster_rbac.py`](../scripts/validate_shared_cluster_rbac.py).

Bridge's selected guest profile names Kubernetes v1.35.0. The validator uses
the v1.35.8 image published with kind v0.33.0, staying in the selected minor
release. Neither input is an approved Bridge production image profile yet.

The validator exercises Kubernetes authorization. It does not install Orka or
test controller reconciliation, stored Session data, network isolation or human
identity-provider authorization.

## Important limits

### A Task creator can indirectly use namespace Secrets

Denying direct `get secrets` is necessary, but it is not a complete Secret
boundary. At the selected Orka revision, a Task can carry Kubernetes environment
variables, including `secretKeyRef`, and the controller creates the Job with its
own authority. A creator who can submit such a Task may be able to expose a
known same-namespace Secret through the Task result or logs.

For the current design, each creator trust domain must therefore be treated as
trusted with effective use of every Secret in its Orka namespace. Credentials
for another creator must never be placed there, and every installation needs
independent control-plane and provider credentials.

A stricter product requirement needs Orka admission and API authorization for
each Secret or privileged resource reference, applied consistently to direct
CRD writes and Orka API writes. Kubernetes RBAC alone cannot inspect object
fields. This restriction must land before Bridge claims that a Task creator is
isolated from the control-plane Secrets in the same namespace.

### RBAC is additive and not field-aware

Kubernetes RBAC cannot provide:

- Per-object creator ownership in one namespace.
- Metadata-only Agent visibility.
- Secret-name-only listing without also permitting Secret data listing through
  the Kubernetes API.
- Field restrictions on images, commands, Secret references, resource limits or
  placement.
- Network, compute, storage, Task-count or model-spend isolation.
- A deny rule that overrides another binding.

Shared clusters also need ResourceQuota, LimitRange, finite Orka Task limits,
an enforcing network policy implementation, model/provider quotas and audited
administrative operations. Those controls are outside this RBAC fixture.

## Validation

The validator creates a uniquely named disposable kind cluster and a private
kubeconfig. It never reads or changes the user's current kubeconfig. It applies
the example roles and runs a `kubectl auth can-i` matrix for two creator
identities and two namespace-local observer identities, including their
ServiceAccount groups. Orka's virtual Sessions and Chat, and uninstalled
dependency resources, retain explicit SubjectAccessReview checks: `can-i`
relies on Kubernetes discovery and otherwise loses their API group. These
reviews check grants only, not the Orka API's behavior or stored records.

It checks that:

- Each creator can manage Agents, Tasks and virtual Sessions in their own
  namespace.
- Each creator is denied the same access in the other creator's namespace.
- Each namespace-local observer can read complete Agents, Tasks and Sessions in
  its installation, but cannot mutate them, read Secrets or cross into the
  other installation.
- Creators cannot directly read Secrets, create workloads, mint ServiceAccount
  tokens, change RBAC, bind or escalate roles, or impersonate identities.
- Two creators bound in one deliberately unsafe shared namespace can both patch
  Agents there. This is the negative control proving that a namespace is the
  ownership boundary.

The only object mutation scenario is the shared-namespace negative control.
Commands use standard-library subprocess timeouts and private file-backed
output, avoiding reader threads and inherited-pipe waits. Failure messages omit
child output entirely, so credential redaction is unnecessary. Output read into
memory is capped; temporary output files are not a disk-output quota.

Prerequisites are Python 3.11 or newer, kubectl v1.35, kind v0.33.0, and a
running Docker or Podman Linux-container provider. Supply the provider
explicitly:

```sh
python3 scripts/validate_shared_cluster_rbac.py --provider podman
```

The node image is pinned by digest in the script. `--kind` and `--kubectl` can
select exact tool binaries. Use `--keep-cluster` only for manual diagnosis; the
default verifies an ownership marker and deletes the cluster when validation
finishes.

Unit tests do not create a cluster:

```sh
python3 -m unittest scripts/test_validate_shared_cluster_rbac.py -v
```

This validation does not replace an integration run with two real Orka
installations. Before Bridge implements shared clusters, that run must also
prove that each controller and runtime identity is denied the other
installation's Secrets and workloads, accounting for the staged chart's
explicit shared-resource and inventory grants; that creator A's token receives
`403` from Orka B; and that namespace-local observer identities can read only
the intended stored Sessions and execution events.

## Bridge implications

Bridge should model each Orka installation as a separate place even when
several places use one Kubernetes cluster:

- Every read and mutation carries an explicit context and namespace.
- Send targets one watched namespace and its matching Orka endpoint.
- Overview fans out only across places the user connected and reports partial
  failures rather than treating a cluster as one authorization domain.
- Role discovery uses authorization checks; it does not infer authority from a
  successful health request.
- Cross-creator inventory and activity use explicit observer bindings and
  namespace-local credentials; those bindings disclose complete Agent, Task and
  Session records in each enrolled installation.
- Bridge never stores or uses the platform administrator or controller
  credential for ordinary creator operations.

Supporting one controller across creator namespaces remains upstream design
work in [orka-agents/orka#630](https://github.com/orka-agents/orka/issues/630).
The namespace-per-installation model does not require that controller change.
