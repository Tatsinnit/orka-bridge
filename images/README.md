# Image assets

Bridge image bytes stay separate from the desktop app. These standard-library
Python tools package one supplied OS disk, verify its chunks, and define a gated
manual workflow for **draft** release assets. They do not build a disk or VM.
Python 3.11 or newer is required; there are no Python package dependencies.

**`bridge-image/v1` is provisional.** The native-runtime owner must accept this
mapping before it is frozen. `publisher-policy.json` intentionally contains no
accepted schema hash or profiles. Neither a caller-supplied manifest nor a
successful fixture test can approve a release. No image is approved for shipping.

## Local packaging and verification

With an owner-supplied disk and metadata file:

```sh
python3 images/package.py --input os.disk --metadata metadata.json --out package
python3 images/package.py --verify package/manifest.json
# Optional local reconstruction; the destination must not exist:
python3 images/package.py --verify package/manifest.json --out restored.disk
```

The default chunk limit is 1 GiB. `--chunk-bytes` accepts positive values up to
that limit for small tests through the same code. Packaging reads at most 1 MiB
at a time and creates deterministic gzip bytes within the same Python/zlib
toolchain. The manifest records ordered chunk names, byte counts and SHA-256,
the whole gzip identity, and measured original byte count and hash. These are
integrity checks against selected bytes, not signatures or source authenticity.

The output directory must be new. A completed package has `manifest.json`,
published only after all chunks finish. Interrupted output can remain partial;
choose a fresh directory, not an overwrite. Verification rejects missing, extra,
symbolically linked, reordered or damaged chunks, invalid gzip and mismatched measurements.
`--manifest-sha256` anchors verification to an independently selected manifest
hash. Without that option, verification checks internal consistency only.
Hash-only verification is the default and needs no second raw disk copy.
Optional reconstructed output must be outside the package directory and is
finalized without replacing an existing file.
A failed reconstruction leaves `<destination>.partial` for inspection; retry with
a fresh destination, rather than automatically deleting or trusting partial bytes.

## Input and manifest mapping

Metadata has exactly these fields: `imageId`, `buildSourceCommit`, `platform`,
`architecture`, `format`, `minimumHostVersion`, `minimumAppVersion`,
`stateCompatibilityId`, `transportProtocolMajor` and `componentLock`.
The last field is the existing [guest input lock](guest/README.md), not a new
producer provenance format. Paths in that input lock are never copied to the
public manifest. The builder commit is distinct from the lock's Orka commit.

Supported pairs are `macos/arm64/raw` and `windows/amd64/qcow2`. Linux is a guest
and reference-test platform, not an added desktop download target. Minimum host
and app versions are owner-supplied compatibility declarations, not measured
memory requirements or proof that a host can boot the guest.

The manifest binds the joint two-disk contract, `H1/v1`:

- Only `os-disk` is a downloadable artifact: a GPT/UEFI factory with 16 GiB
  virtual capacity. Each installation uses its own writable copy, identified as
  `bridge-os`; its later modified hash is not the factory download hash.
- `state-disk` is only a `create-empty` descriptor, with 32 GiB virtual capacity,
  whole-device ext4 and identifier `bridge-state`. The host creates it privately
  using the target format. `new-only` describes the policy, not permission to
  format an arbitrary blank or existing disk; live transaction authorization is
  still required. No initialized state payload is packaged.
- Devices bind virtio disks, NAT, fresh entropy, UTC, the private nonconsole port
  `ai.orka.bridge.control` and separate diagnostics. The transport major is 1
  with JSON-line framing. H1 also requires early persistent identity and volatile
  credentials before container services; this tool does not implement that boot.
- `stateCompatibilityId` comes from the owners. Four vCPU and 6 GiB guest RAM
  are engineering test settings. Private installation UUIDs, MACs, boot nonces,
  credential identifiers and host paths are not release-manifest fields.

Declared virtual capacity is separate from measured payload file bytes. Tiny
fixtures can carry the contract while remaining explicitly **non-bootable**;
these tools do not parse GPT, EFI or QCOW2 to certify an actual disk.

`components` records the guest lock's semantic input identity, canonical lock
byte hash and exact checked-in version-policy hash as separate identities.
Kubernetes is explicitly **policy-declared**, not covered solely by the semantic
lock hash. Orka, kind, Vekil and selected tools retain their available identities.
kmx is currently represented by its nullable profile checksum; this does not
invent an executable version/hash or assert that the guest endpoint exists.
`publicationReady` stays false. See [the draft schema](manifest.schema.json).

## Gated draft workflow

The checked-in profile must first pin the accepted schema bytes, repository,
workflow, successful build run, source commit, artifact ZIP hash and selected
payload/metadata member hashes, target, input identity and reviewed native-receipt
identity. No producer member names or qualified build workflow are guessed here.
The guest's existing production input policy must approve the actual lock too.
A profile is a reviewed repository change, not workflow input that self-approves.
The empty policy rejects before any network call.

The manually selected profile is checked on trusted `main`. Prepare has read
permissions only. It first requires the builder commit's checked-in guest version
policy to match the publisher's exact policy bytes, so policy-declared Kubernetes
cannot silently refer to a different build policy. It validates run and artifact
identities, cross-binds metadata
to the same source and target, then streams the selected ZIP member directly
into packaging. It never extracts a raw disk or runs downloaded code. Plan for
ZIP plus compressed-package space; the draft roundtrip needs two compressed
package copies. Available space and real input sizes must be checked before
using a qualified runner; a standard runner is not promised to fit every image.

Only the separate draft job has release-write permission. It requalifies the
same bytes against prepare's manifest hash, serializes the image's derived tag,
rejects existing tags and all matching releases including drafts, and checks its
new draft is unique. Any tag appearing during the operation must point directly
to the qualified source commit; other tag identities fail closed. Uploads never
clobber. It redownloads through authenticated
asset IDs and verifies against the original manifest hash. Failure leaves the
draft for operator inspection, never silently retries or publishes it. A draft
is not release approval. Freezing the schema, native boot results and a separate
publication decision remain required. This PR must not be used to dispatch or
publish an image before those handoffs.

## Tests

```sh
python3 -m unittest discover -s images -p 'test_*.py' -v
python3 images/package.py --help
python3 images/qualify.py --help
actionlint .github/workflows/images.yml
```

Tests use small non-bootable payloads and synthetic ZIPs. GitHub API calls are
mocked at that boundary, including every draft mutation; no tests publish.
CI runs unit tests without external networking. Actual builder artifact download,
draft API mutation/roundtrip and native boot are not proved by these fixtures.
