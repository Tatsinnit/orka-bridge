# Contributing to Bridge

See the [README contribution terms](README.md#contributing) for the Microsoft
CLA and Code of Conduct.

## Development

Use Node 26.5.0 and npm 11.17.0. Rust is pinned to 1.98.1 in
`app/src-tauri/rust-toolchain.toml`. Install the
[Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/) for
your operating system.

From `app/`:

```sh
npm ci
npm run tauri dev
```

Dependency lifecycle scripts are disabled in `.npmrc`. The pinned optional
dependencies supply platform binaries; do not enable scripts globally to install.

## Engine diagnostics

Clusters reports engine availability, not cluster readiness. The default VM
transport is not implemented yet; opening the app does not require kmx.
For development only, set `BRIDGE_DEV_KMX_PATH` to a kmx executable before
`npm run tauri dev`. Release builds ignore this override and cannot select
the development process transport. No helper is discovered or downloaded.

The native bridge accepts only version and read-only Agent list requests.
Replies contain validated version or identity fields, not raw diagnostics.
No credential input channel is provided here; runtime integration must define
that separately. Closing the main window signals cancellation. While the app's
async runtime is running, cancellation kills and reaps the owned direct process;
app exit does not wait for that cleanup. Neither grandchildren nor remote work
are guaranteed to stop.

Run debug engine tests and clippy as well as release checks: process transport
code is compiled only when debug assertions are enabled.

## Checks and builds

Run the commands in [AGENTS.md](AGENTS.md#checks). Also run `npm audit` to
inspect development-tool advisories; `npm audit --omit=dev` checks the shipped
frontend dependency graph. Do not apply forced major-version audit fixes blindly.

For a Windows x64 executable:

```sh
npm run tauri -- build --no-bundle -- --locked
```

For an Apple silicon Mac app:

```sh
npm run tauri -- build --bundles app --no-sign --no-binary-patching -- --locked
```

The Mac bundle includes readable notices in its Resources directory; CI also
places them beside the portable Windows executable. Both builds embed them in
the frontend. Preserve these files when redistributing builds.

The app has three empty screens and an engine diagnostics panel. It does not install a runtime,
save agents, connect to clusters or run tests. There is no app data store;
native webviews can still create their usual cache/profile directories under
the app identifier `ai.orka.bridge`.

CI artifacts expire after three days. They are test builds, not a release:
no installer, Developer-ID signing, notarization or updater is provided.
Browser-downloaded Mac artifacts may be blocked by Gatekeeper. Windows needs
WebView2. Mac archives preserve executable permissions.

## Native smoke walk

The Mac CI job launches its extracted app bundle, uses pointer and keyboard
navigation, checks captured page headings and resizes the window to its
minimum dimensions. In Clusters it also checks the actual release engine IPC
reports unavailable. The walk runs in both system themes, temporarily enables
macOS Keyboard Navigation, then restores the runner's prior preferences. Captures contain only the app window.
Review visible changes in these captures and report anything not exercised,
such as platform input feel or installer behavior.
