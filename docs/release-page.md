<!-- App release drafts use this file as their text. Update "New in this release" in each version PR. -->
Bridge runs an AI agent on your own computer and checks its answer with a test. It sets up a small Linux VM with [Orka](https://github.com/orka-agents/orka) inside, so you don't install Kubernetes, Docker or Orka. This is a preview for Apple-silicon Macs (macOS 13 or later) and Windows 10 or 11 on x64.

**Download one file:** `bridge-macos-arm64.tar.gz` for a Mac, or `bridge-windows-x64.zip` for Windows. You don't need `bridge-windows-runtime-source.zip` (about 900 MB): it's the source for the VM engine bundled in the Windows app, published for its license. Bridge downloads its VM image itself.

**Before you start**

- GitHub CLI, signed in to an account with Copilot (`gh auth login --hostname github.com`). Bridge copies this sign-in into its VM, so use an account you're comfortable sharing with it.
- Mac: if GitHub CLI isn't installed, download the `.pkg` from the [GitHub CLI releases page](https://github.com/cli/cli/releases/latest). macOS can't verify that installer either, so open it once, then choose Open Anyway in Privacy & Security; on an account that isn't an administrator, both that step and the install ask for an administrator's name and password.
- 16 GB of memory recommended; the VM uses 6 GB.
- About 75 GB free disk on a Mac, or 60 GB on Windows.
- Windows: turn on **Windows Hypervisor Platform** in Turn Windows features on or off, and install the [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) if it's missing.

**Install and first open.** The app isn't signed by a known developer, so your computer asks once:

- Mac: open the download in Finder and move Bridge to Applications, or to an Applications folder in your home folder if you aren't an administrator. Open it once; macOS says it can't verify Bridge. Then open System Settings > Privacy & Security, choose Open Anyway and confirm Open. If your account isn't an administrator, macOS asks for an administrator's name and password: ask whoever manages your Mac, or your IT team.
- Windows: use Extract All, keep the whole folder, and open `orka-bridge.exe` from it, not from inside the ZIP. If SmartScreen says it protected your PC, choose More info, then Run anyway.

If macOS says Bridge is damaged, or offers no Open Anyway, stop and [report it](https://github.com/orka-agents/orka-bridge/blob/main/docs/preview.md#report-a-problem). Don't turn off Gatekeeper.

Most of the setup time is a 2 GB download. Follow the [setup guide](https://github.com/orka-agents/orka-bridge/blob/main/docs/preview.md).

**New in this release**

- A Mac download opens: earlier Mac releases showed "damaged" in a browser download.
- Clearer setup steps and failed-check messages.
- Overview lists your agents in one place, and shows the workspace's health.
- Bridge uses the Orka website's look.
