# Try the Bridge local preview

Bridge starts a Linux virtual machine (VM) on your computer with Orka, the
software that runs AI agents. You can connect GitHub Copilot, open an example
agent that fetches a web page, and test its answer and tool use.
No separate Kubernetes, Docker or Orka installation is needed.

**Before connecting:** Bridge copies your GitHub CLI sign-in token into the
VM for Copilot and saves its own copy in your operating system's credential
store. This is your GitHub token, not a separate token limited to this preview.
Only connect an account whose credentials you are comfortable giving to the VM.
[Remove sign-in](#remove-bridge) clears Bridge's copies; it does not revoke the
token or sign GitHub CLI out.

This unsigned preview is for invited testers on Windows x64 and Apple-silicon
Mac. It is local-only: don't use **Send to a cluster** or existing-cluster
setup. The VM runs locally, but model requests go to Copilot over the internet.

## Prerequisites

- Windows 10 or later on x64, or macOS 13 or later on Apple silicon.
- The VM uses 6 GiB RAM and four virtual CPUs; leave memory for other apps.
- Windows: enable hardware virtualization and **Windows Hypervisor Platform**
  in **Turn Windows features on or off**; restart if asked. Ask your
  administrator if needed. Bridge uses WHPX, not WSL.
- Windows: install the [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)
  if missing. The app includes its QEMU VM engine; don't install QEMU separately.
- Install [GitHub CLI](https://cli.github.com/) and sign in to `github.com`
  with an account that has Copilot access. In a terminal, run
  `gh auth login --hostname github.com`, then check the account with
  `gh auth status --hostname github.com`. Bridge doesn't sign you in itself.
- Internet access to GitHub downloads and Copilot. Images download anonymously
  from public GitHub assets; only model connection uses your token.

### Disk space

Budget **60 GiB free on Windows** or **75 GiB on Mac** on the drive holding
your user profile. This includes room for the app, cached image, private VM
copy, disk growth and working space. If you download the app to another drive,
that drive also needs room for its archive and extraction.

Approximate sizes, rounded from measured packages:

| Item | Windows | Mac |
| --- | ---: | ---: |
| Image download | 1.84 GiB | 2.23 GiB |
| Image cache and private copy, before VM writes | 8.91 GiB | 34.30 GiB |
| App archive / unpacked files | 53 / 425 MiB | 9 / 29 MiB |

The sparse 32 GiB state disk grows as used. Windows's system disk can grow
to 16 GiB; Mac's is already 16 GiB. Budgets allow for this growth.
Use the app release's budget if revised there.

## Open the app

Use the exact app release link in your invitation, in
[Bridge releases](https://github.com/orka-agents/orka-bridge/releases).
A **Bridge VM images** release is not an app: Bridge downloads those images
itself. The Windows runtime source archive isn't needed to run Bridge.
See the [release notes](release-notes.md) for the pinned Orka version.

Unsigned apps trigger security warnings. Only make an exception for the app
you meant to download from this repository; keep system protections enabled.

### Windows

1. Download `bridge-windows-x64.zip`. Use File Explorer's **Extract All** into
   a writable folder. Keep the whole folder, including `native\qemu` and notices.
2. Open `orka-bridge.exe` from that folder, not from inside the ZIP.
3. If SmartScreen says **Windows protected your PC**, choose **More info**,
   check the executable, then **Run anyway**. If that option is absent or
   another policy blocks it, [report the block](#report-a-problem) rather than
   disabling protection.

### Mac

1. Download `bridge-macos-arm64.tar.gz` and open it in Finder to extract
   `Bridge.app`. Move it to Applications or your own Applications folder.
2. Open the app. If blocked as unidentified or unnotarized, open **System
   Settings > Privacy & Security**, find Bridge and choose **Open Anyway**.
   Confirm **Open** and authenticate if asked; wording varies by macOS version.
3. If no exception is offered, or macOS reports malware or a damaged app,
   [ask for help](#report-a-problem). Don't remove quarantine or disable Gatekeeper.

Apple's [per-app exception instructions](https://support.apple.com/en-us/102445)
and Microsoft's [SmartScreen guidance](https://learn.microsoft.com/windows/apps/package-and-deploy/publish-first-app)
describe these prompts.

## Set up and run the example

1. Open **Clusters**. Choose **Get started** on the first visit, then
   **Set up** on **This computer** and **Continue**.
2. Leave Bridge open while it downloads, verifies and installs the image,
   then starts the VM. You don't need to choose an image or download URL.
3. Once ready, choose **Use GitHub sign-in** in the model section, having
   read the token warning above. Wait for the model to connect.
4. Choose **Read its instructions** to open `web-reader` in **Designer**, the agent
   editor. Its files live in `Orka/agents/web-reader` under your home folder.
5. Choose **Run tests** on **Tests on the local cluster**. Save any edits before running.
   For the unchanged example, look for **1 of 1 passed**.
   Expand its **Passed** result to read the answer and check **Tools used**
   includes `web_fetch`. “Model connected” alone doesn't prove inference.
6. Report a failed or unknown result. Use **Copy details for help** if shown
   and note the last step that worked.

## Stop and reopen

Closing the window hides Bridge; the workspace keeps running. Use the tray or
menu-bar icon's **Open Bridge**, or launch the app again, to reopen the window.
Choose **Stop the local cluster and quit** from that icon's menu to stop it completely.
Wait for quitting to finish before moving or deleting files.

Quitting keeps VM files and your saved sign-in. On reopening, Bridge reuses
its workspace; **Use GitHub sign-in** reuses Bridge's saved copy.

## Remove Bridge

1. While the workspace runs, choose **Remove sign-in** in **Clusters**' model
   section. Wait there for **Bridge no longer has a saved sign-in. GitHub CLI
   stays signed in.** This stops the model connection and clears the VM and
   OS-store copies. If it fails, retry the same button. If it still fails,
   ask for help before deleting the VM or its metadata.
2. Choose **Stop the local cluster and quit**; closing the window isn't enough.
3. Delete the extracted Windows app folder, or move `Bridge.app` to Trash.
   Remove the downloaded archive too if you no longer need it.
4. Open the following folder in File Explorer's address bar, or Finder's
   **Go > Go to Folder**. Delete `local-vm-v1` and `image-cache-v1` inside it
   to permanently remove the workspace disks and cached images:

   - Windows: `%LOCALAPPDATA%\ai.orka.bridge`
   - Mac: `~/Library/Application Support/ai.orka.bridge`

5. Optionally remove native preferences and history by deleting the whole
   folder above instead. On Windows also delete `%APPDATA%\ai.orka.bridge`.
   Deleting folders doesn't replace **Remove sign-in**.
   Your agent files under `Orka/agents` are separate:
   keep or back them up; delete them separately only if unwanted.

## If something's different

With multiple GitHub accounts, use `gh auth switch` to select the active
`github.com` account before connecting. Bridge reads the stored sign-in,
not `GH_TOKEN` or `GITHUB_TOKEN` overrides. Its saved copy takes precedence;
use **Remove sign-in** before switching the account used by Bridge.
Supported GitHub CLI locations are the 64-bit Windows Program Files folder's
`GitHub CLI\gh.exe`, or `/opt/homebrew/bin/gh` and `/usr/local/bin/gh` on Mac.

If an unrelated `Orka/agents/web-reader` folder exists, Bridge reports an
error rather than overwriting it. Ask for help; don't delete files you need.

## Report a problem

Contact your inviter or open a [Bridge issue](https://github.com/orka-agents/orka-bridge/issues/new/choose).
Include OS and CPU type, app release link, failed step, exact error and a
screenshot if useful. Say whether a download failure involved the app or VM
image; for tests, include the failed case.

Remove tokens, private agent content and personal paths before sharing.
Never attach credentials, VM disks or a whole app-data folder.
