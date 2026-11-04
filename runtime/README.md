# Guest endpoint

The separate Linux-only `bridge-guest` package shares the pure protocol crate,
not Tauri. It has only `--early`, `--serve` and harmless `--help` modes.
Readiness means validated identity, mounts and endpoint, not cluster or Orka health.
Engine execution, chunks, credentials and run-file operations are not implemented.

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
`/run/bridge/boot.json`. Credentials have a distinct private tmpfs mount.
The image owner must carry that entire initramfs `/run` across switch-root,
then guard these mounts before every Docker and containerd start or restart.
Normal mode revalidates the saved descriptor, mounts and host's Existing
reaffirmation before readiness. It never mounts or formats.

Both modes use the root-owned, mode-0600 character port
`/dev/virtio-ports/ai.orka.bridge.control`. Stale bytes are boundedly drained before
each fresh stage hello. Only protocol frames use this port; errors are generic
on stderr. Serve creates a mode-0600 `/run/bridge/kmx.sock` under a private
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
