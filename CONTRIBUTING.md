# Contributing to Bridge

See the [README contribution terms](README.md#contributing) for the Microsoft
CLA and Code of Conduct.

## Build and run

Install Node 26.5.0, npm 11.17.0, Rust 1.98.1 and the
[Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/) for your
operating system. Install Rust through rustup so the `cargo +1.98.1` checks work.
Mac builds also need the Xcode Swift compiler.
Rust is pinned in `app/src-tauri/rust-toolchain.toml`.

From the repository root:

```sh
cd app
npm ci
npm run tauri dev
```

The default window is 1280×800, with a 540×640 minimum. The native smoke walk also captures a 540×900 content area in both system themes.

This opens the native app with frontend hot reload. You can edit the UI without
a VM; running an agent needs the local development image described below.
Dependency lifecycle scripts are disabled in `app/.npmrc`. Do not enable them
globally to install dependencies.

## Run with a local VM

Local VM development needs a debug build on Windows x64 or Apple silicon Mac.
Set `BRIDGE_DEV_IMAGE_DIR` to a folder containing all four matching files:

- `boot-assets.json`
- `os.kernel`
- `os.initrd`
- `os.qcow2` on Windows, or `os.raw` on Mac

The files must match the sizes and SHA-256 hashes in
[`dev_image.rs`](app/src-tauri/src/dev_image.rs). Do not mix files from different
builds or convert the disk format. Ask a maintainer for a matching development
image. [The guest image build guide](images/guest/README.md) describes image
preparation, but does not reproduce these pinned files. Bridge installs private
copies; it does not modify this source folder. A previously installed compatible image can reopen without it.

### Windows x64

Enable Windows Hypervisor Platform. Bridge uses WHPX, not WSL or a host QEMU
installation. Extract `bridge-windows-runtime-portable.zip` to `native\qemu`
beside the executable. Its `provenance\runtime-manifest.json` must match the pin
in [`local_vm_native.rs`](app/src-tauri/src/local_vm_native.rs). Keep the tree
and notices intact. See [the Windows runtime producer](runtime/windows/README.md)
for artifact download instructions.

From `app/`, in PowerShell:

```powershell
$env:BRIDGE_DEV_IMAGE_DIR = 'C:\path\to\image'
npm run tauri -- build --debug --no-bundle -- --locked
```

Still in `app/`, place the QEMU tree at `src-tauri\target\debug\native\qemu`,
then launch the app:

```powershell
.\src-tauri\target\debug\orka-bridge.exe
```

The `bridge-windows-x64` CI artifact includes an embedded-frontend debug build
at `development\orka-bridge.exe`. Place QEMU at `development\native\qemu`, set
`BRIDGE_DEV_IMAGE_DIR` in PowerShell, and run `development\orka-bridge.exe`
from that shell. The executable at the artifact root is a release build and
cannot use the local development image.

### Apple silicon Mac

From `app/`:

```sh
BRIDGE_DEV_IMAGE_DIR=/path/to/image npm run tauri dev
```

Mac builds compile and ad-hoc sign the bundled VM helper automatically.
See [the helper build instructions](app/src-tauri/native/macos/README.md).
Intel Macs can build the app but cannot run its local VM. Both pinned images
support model Connect, the workspace catalog and the example agent.

In Clusters, dismiss the one-time welcome with Get started, then press Set up
on This computer's place and Continue after its computer checks. On Windows and
Apple-silicon Mac, the running VM unlocks Use GitHub sign-in, which needs GitHub CLI already installed and signed
in to github.com. Bridge does not install it or open browser sign-in. Starting
a VM alone does not verify model inference. Local images are debug-only;
release builds ignore `BRIDGE_DEV_IMAGE_DIR`. Supported release builds select
reviewed hash-pinned images from the image-only prerelease and download them
anonymously. Before that prerelease is published, startup reports a missing-image
download error; it never falls back to GitHub credentials. A release candidate
still needs the [preview walks](docs/preview.md) before app publication.

## Reopen and remove the saved sign-in

After **Stop the local cluster and quit**, reopen Bridge and use **Use GitHub sign-in**
once the workspace is running. Bridge reuses its native-store copy without
reading GitHub CLI again. A new VM boot first disconnects the prior owned model
route. This requires an image built with the retained-connection cleanup handler;
older development images cannot reconnect across boots. Build and pin the new
guest together with its matching app and a new source-bound state compatibility
ID. New guest replies are not supported by older app builds.

While the workspace is running, choose **Remove sign-in** in the model section
before removing Bridge. This stops the owned model route, clears its guest
credential and removes only Bridge's native-store copy. You can connect again.
GitHub CLI remains signed in; removal is not token revocation. A failed or
unknown guest cleanup keeps the saved sign-in for another explicit attempt.
An interrupted first Connect with no acknowledged connection identity, or a
completed disconnect whose host update was lost before another VM boot, may
still require maintainer help. Bridge never guesses an identity or deletes
metadata to bypass cleanup.

## Checks

From `app/`, run the [contributor checks in AGENTS.md](AGENTS.md#checks).
Build frontend assets before Rust checks because Tauri embeds them. In addition,
check the debug-only engine and local-image code:

```sh
cargo +1.98.1 test --manifest-path src-tauri/Cargo.toml --locked --test engine
cargo +1.98.1 test --manifest-path src-tauri/Cargo.toml --locked --lib --test dev_image_profile
cargo +1.98.1 clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
npm audit
npm audit --omit=dev
```

Do not apply forced major-version audit fixes blindly. CI runs frontend checks
and native Windows and Mac builds. Its Mac smoke walk captures navigation in
both system themes under `native-walk/` in the `bridge-macos-arm64` artifact.
Review those captures for visible changes; they do not exercise a real VM or
model connection.

## Platform builds

From `app/`, build a Windows x64 release executable:

```sh
npm run tauri -- build --no-bundle -- --locked
```

For an Apple silicon Mac app bundle:

```sh
npm run tauri -- build --bundles app --no-sign --no-binary-patching -- --locked
```

Preserve the bundled third-party notices when redistributing builds. Mac bundles
include them in Resources; Windows artifacts place them beside the executable.
CI artifacts expire after three days. They are unsigned test builds, not
installers or releases. Windows needs WebView2; browser-downloaded Mac builds
may be blocked by Gatekeeper.

## Where things live

- `app/src/`: React UI and frontend tests.
- `app/src-tauri/`: Rust shell, native commands and tests.
- `app/src-tauri/native/macos/`: Swift VM helper.
- `runtime/`: VM protocol, guest endpoint and Windows QEMU build.
- `images/`: Guest image build, packaging and validation.
- `docs/`: Roadmap and shared-cluster design.
- `.github/workflows/checks.yml`: App checks, artifacts and native smoke walk.

Keep renderer input separate from native credentials and VM ownership. Preserve
Microsoft project files and third-party notices, and do not edit generated
lockfiles by hand.
