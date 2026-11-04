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

The app currently opens three empty screens. It does not install a runtime,
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
minimum dimensions. The walk temporarily enables macOS Keyboard Navigation,
then restores the runner's prior setting. Captures contain only the app window.
Review visible changes in these captures and report anything not exercised,
such as platform input feel or installer behavior.
