# Guest endpoint

The separate Linux-only `bridge-guest` package shares the pure protocol crate,
not Tauri. It has only `--early`, `--serve` and harmless `--help` modes.
Boot's Ready acknowledgment validates identity, mounts and the endpoint, not Orka health.
Post-boot Readiness also reports the matching bootstrap phase and current infrastructure
service state. Native startup waits for `infrastructure-ready` and successful service
completion before unlocking model setup. Older storage-only replies cannot satisfy this check.
Serve accepts digest-committed agent/test snapshots, typed kmx Version and
ListAgents actions, and bounded native Run/Chat Orka requests. The Chat contract
below does not enable generic command execution or a general HTTP proxy.

## Files and typed operations

The single desktop client lives in `app/src-tauri/src/runtime/client.rs` and is
consumed by the VM engine adapter; later setup must extend it, not add another
client. It accepts a serving channel only after the owned R1 handshake. Native
channel implementations must enforce I/O deadlines. Default app transport remains
unavailable; this does not launch a VM.

Inputs are only the explicitly selected agent/test pair, at most 1 MiB each,
with no symlink or parent traversal. Capture walks relative to held directory
handles with cap-std/cap-fs-ext, including Windows; it never reopens an admitted
ambient path. Capture rereads both files, retains immutable
bytes and binds both SHA-256 digests. Guest chunks are sequential and at most
32 KiB; commit verifies both digests before atomically installing the snapshot.
There is no shared folder or arbitrary archive extraction. The state store caches
at most 32 committed snapshots and 32 result payloads, evicting older cached bytes
without stopping future reads. Nonsecret operation metadata is retained for deduplication;
eviction never permits execution of an old ID. Historical results may become Unknown
once their payload expires. Future file-execution support must pin active input bytes.

Operations record their caller ID before fixed `/usr/local/bin/kmx` execution.
Argv uses the existing version and scoped JSON agent-list commands; no shell or
caller environment is accepted. Guest HOME is fixed to `/root`. ListAgents uses
bootstrap's state-disk `KUBECONFIG=/var/lib/bridge/kubeconfig` and
`KMX_HOME=/var/lib/bridge/kmx`; Version keeps `/root/.kube/config` and no KMX_HOME.
Image integration must supply the pinned kmx and its helper dependencies. Output
is bounded; only version/Agent metadata is persisted, not CRD specs or raw stderr.
Results are ID-bound chunks, never guest-selected paths or writes over creator
files. A lost response stays Unknown until an explicit ID lookup; duplicate IDs
never reexecute. Interrupted active records become Unknown.
The kmx actions remain read-only: no file execution flags, Task/package lookup
or cancellation is inferred from incomplete upstream machine-output work. Native
Orka requests use their separate guest-owned transport.

## Native Run cancellation observations

The physical host channel accepts Run `orka` requests to
`tasks/bridge-task-<32 lowercase hex>/cancel`. This is a POST with exactly
`{"uid":"<acknowledged Task UID>"}` (nonempty, at most 64 bytes), no event cursor.
HTTP status and JSON are preserved, including 200, 409 and denials; the guest
never retries the request. Run still cannot DELETE resources.

The typed `runWorker` action accepts only a generated Run Task `name` and its
acknowledged `uid`. It reads that Task, refuses a missing or mismatched UID,
then lists pods labelled `job-name=<status.jobName>` in `orka-system`. Both reads
use the fixed guest kubeconfig and impersonate `bridge-native-run`. No bearer
is minted for this observation, and no caller-selected namespace, selector,
command, pod spec or log is exposed. Only `{finished: bool, pods: number}` is
returned and persisted. `finished` means the Task is Succeeded, Failed or
Cancelled. `pods` counts every pod except those in Succeeded or Failed,
including terminating pods in other phases. Completed pods may remain for
results. Cancellation proof requires **both** `finished` and `pods == 0`.
An absent Job name gives zero pods; a nonterminal Task still gives
`finished: false`, as before scheduling or during a Pending retry. This
observation is for native Run's AI Tasks, which disable retries and do not use
autonomous iterations; it does not track workers from earlier Job attempts.
Failed or incomplete reads are not zero counts. Neither operation is available
through the local guest socket.

Run's Role adds namespace-wide `update` on `core.orka.ai` `tasks/cancel` and
`list` on core `pods`; validated guest requests restrict reachable Run names.
Chat's Role is unchanged. This requires a rebuilt guest image, Orka with the
cancel API, and a fresh local cluster. Identity setup never rewrites an existing
Role: a new guest encountering the old Role refuses all Run calls rather than
expanding its permissions. These guest operations alone add no host cancellation
loop or UI, and do not update an installed image.

## Native Chat transport

The physical host channel also accepts the separate `orkaChat` action. Its request
has an explicit `method` (`get`, `post`, `delete`), `path`, optional event cursor
`after`, and optional JSON `body`. Run's existing `orka` schema and identity do not
accept DELETE. Neither action can be invoked through the in-guest local socket or
translated into a kmx command.

Chat creates generated `bridge-chat-agent-<32 lowercase hex>` Agents and
`bridge-chat-task-<32 lowercase hex>` native AI Tasks in `orka-system`, with
`bridge-chat-<32 lowercase hex>` conversation references. It reads those named
Agents, Tasks, Task results/events and Sessions. Only generated Chat Tasks may be
deleted; lists, arbitrary paths/URLs and Agent/Session deletion are unavailable.
Task references must name Chat resources; an Agent reference's optional namespace
must be `orka-system`. Other Agent/Task configuration remains the native caller's
responsibility, as with Run.

The lazy guest-owned `bridge-native-chat` identity has separate credentials and
forward state from `bridge-native-run`. Its namespaced RBAC permits Agent
create/get, Task create/get/delete, Session get and provider/tool reads. Kubernetes
Task deletion permission is namespace-wide: validated Chat paths and the bearer
remaining inside the guest, not RBAC name restrictions, provide Chat-only access.
Identity setup reuses only exact guest-owned manifests, never rewrites Run's Role,
and never sends bearer credentials to the host. Requests retain the existing
64 KiB create-body and 128 KiB frame bounds, no redirects or automatic HTTP
retries. Both the raw reply and its status-wrapped JSON must fit 1 MiB, so a
near-limit raw response can still be withheld with an Unknown operation outcome.
Oversized replies and bearer echoes are withheld before result persistence.

A DELETE acknowledgement is only a deletion request acknowledgement, not evidence
that the worker terminated. Preserve HTTP status and JSON for the host to report
best-effort Stop with execution termination unconfirmed. Task deletion removes
its stored result and events; it cannot undo tool effects. Operation-ID replay
returns the recorded outcome without repeating the request. Generated Agents and
Sessions are not cleaned up by this transport. Submitted message bodies can also
remain in the VM's private operation records; quitting the app is not erasure.

This is guest/protocol groundwork only: it does not enable Designer Chat, provide
host conversation state, or ship a new VM image. Using it requires a rebuilt guest
image; older guests do not understand the action. The host follow-on must detect
an unsupported image, not fall back to another credential or transport authority.

## Image integration boundary

Early mode is for Debian initramfs-tools after the OS is mounted at the fixed
`/root`, before systemd. The image hook must fail boot on a nonzero endpoint exit.
The image must pre-create mount targets `/var/lib/bridge`, `/var/lib/docker`,
`/var/lib/containerd` and the regular file `/etc/machine-id`. With those present,
the future root may remain read-only; missing targets are a boot error, not
permission to change the image or retry formatting. The state role resolves through `/dev/disk/by-id/virtio-bridge-state`; no guessed
disk ordering is used. It must expose 32 GiB. Formatting requires the host's New
descriptor and a successful blank-signature probe. Existing or ambiguous state
is never formatted. A matching ext4 filesystem without its identity journal is
retained for explicit recovery, not regenerated.

Persistent `machine-id`, Docker and containerd directories bind from state into
the future root. Identity uses `bridge-state/v1` with `installationId`.
The accepted top-level BootDescriptor is written only after guards succeed to
`/run/bridge/boot.json`. Credentials have a distinct tmpfs mount: a private `root`-owned underlay
directory is mounted visible as `root:65532`, mode 0750, `nosuid,nodev,noexec`;
no-follow directory metadata (owner, group, type, mode bits) is checked after
mount and again at every readiness check, alongside the existing exact-tmpfs
check, and a mismatch fails closed rather than being repaired. Token delivery
is not implemented here. A token producer must create and replace files as
`root:65532`, mode 0640; the base image contains no token files or values.
The image owner must carry that entire initramfs `/run` across switch-root,
then guard these mounts before every Docker and containerd start or restart.
Normal mode revalidates the saved descriptor, mounts and host's Existing
reaffirmation before readiness. It never mounts or formats.

Both modes use the root-owned, mode-0600 character port
`/dev/virtio-ports/ai.orka.bridge.control`. Stale bytes are boundedly drained before
each fresh stage hello. A transient HUP-only wait before the first Hello byte
can wait for the host-open notification within the original 120-second deadline.
Reads, other frames and a partially written Hello keep terminal HUP semantics;
this never reopens a channel or replays bytes. Only protocol frames use this
port; stderr diagnostics contain fixed labels, error kind and errno. Serve creates a mode-0600 `/run/bridge/kmx.sock` under a private
directory and checks Linux SO_PEERCRED for UID 0. Existing socket names are not
deleted to make startup work. The original socket is cleaned on normal exit.
Serial loss ends this session; do not restart serve within the same host lease.
A silent root local client can delay serial handling until its bounded frame
read expires. The later adapter must not hold an idle connection.
Graceful shutdown requests fixed systemd poweroff; its reply is not proof of
hypervisor completion. The host must still observe its exact backend stopping.

The image must supply fixed helpers `/sbin/blkid`, `/sbin/blockdev`,
`/sbin/mkfs.ext4`, `/bin/mount`, `/usr/bin/findmnt` in initramfs with their
dependencies, and `/usr/bin/systemctl` in the normal OS. Early mode needs
virtio block/console/entropy, role symlinks, root-only port permissions and
a writable tmpfs `/run`. Copying a static endpoint alone does not provide these.
Image packaging, switch-root, daemon ordering and native VM validation are
separate integration gates, not proved by adapter tests.

## Build-only artifacts

Rust 1.98.1 builds both x86_64 and aarch64 musl targets with rust-lld and
self-contained static CRT. The read-only CI runs tests, inspects actual ELF
architecture, rejects an interpreter or shared dependencies, and records
source commit, compiler identity and hash, lock hash and binary hash.
Artifact records use `bridge-guest-binary/v1`, protocol major 1 and H1/v1.
They are not signatures, production approval or boot receipts.
Artifacts expire after three days; preserve required inputs before expiration.

Run `cargo +1.98.1 test --manifest-path runtime/Cargo.toml --workspace --locked`
and `python3 -m unittest discover -s runtime -p test_artifact.py -v`.
See the workflow for exact build and provenance commands. Never execute early
or serve against a host to test it; unit adapters perform no real mounts,
formatting, Docker startup or poweroff.
