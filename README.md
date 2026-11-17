<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/bridge-mark-dark.svg">
  <img width="128" height="128" alt="Bridge logo: an orca fin above the waves" src="assets/bridge-mark.svg">
</picture>

</div>

# Bridge

**Build and test an agent locally, then deploy the same agent to your team's [Orka](https://github.com/orka-agents/orka) cluster.**

Building an agent for Orka today starts with setting up a cluster, installing Orka, connecting a model, and learning
the YAML. Bridge handles that setup so you can focus on the agent itself.

Invited to the local preview? Start with the [tester guide](docs/preview.md).

## The first hour

Open Bridge and it starts a local Orka environment for you, or connects to a cluster you already have. An example agent is ready to use, with a prompt, the
`web_search` tool enabled, and a test that requires a search to pass.

Click Run to execute the test on your Orka, local or connected. You'll see whether it passed, along with the tool calls. Change the
prompt, add your own tests, and run them again.

When you're happy with the result, choose your team's cluster and click Send. Bridge checks that the cluster supports
your agent, runs the same tests there, and keeps the agent there if they pass.

The local environment makes development easier, but the agent definition, tools, and tests are the same ones used on
the destination cluster.

The [roadmap](docs/happy-path.md) walks through these steps, where each one stands, and open design questions for each.

### Setup

Bridge has three tabs: Overview, Designer, and Clusters.

On first launch, Clusters asks one question: start a local VM, or connect an existing cluster that has Orka on it.
For the local VM, Bridge downloads and starts a VM image with Orka already installed, connects a model, and runs a
test task to verify everything works. Run in the Designer then targets whichever you chose: the local VM, or the
cluster you connected. The existing-cluster choice is planned to land first; the local VM choice follows once the
VM image is built.

Setup happens on a single screen with progress indicators. If something goes wrong, Bridge shows what's missing.

### Designer

Designer opens with an example agent and four sections:

- **Prompt:** Write the agent's instructions.
- **Tools:** Choose from Orka's built-in tools or Tool resources available on the target cluster. Tool resources let
  you define custom tools, such as HTTP endpoints.
- **Tests:** Run real tasks on your local Orka, with pass/fail results, tool calls, execution time, and token usage.
- **Chat:** Interact with the same agent in a live session.

Each agent is defined in a Markdown file, stored either in a Git repository Bridge creates or in one you already have.

### Clusters

Deploy to any Kubernetes context with Orka installed, or let Bridge create a new AKS cluster for you.

Before deploying, Bridge checks the destination's Orka version, model availability, and required tools. It then runs
the agent's tests on that cluster, publishes the agent package to a container registry, and deploys it.

After deployment, Bridge shows where each agent is running, its test status, available tools, and recent runs.
Overview brings that information together across your local environment and connected clusters.

The proposed namespace and access model for team clusters is described in
[Shared Orka clusters](docs/shared-clusters.md). Bridge does not configure these roles today.

## How it works

- **Desktop app:** Built with Tauri 2 for Windows, macOS, and Linux. Bridge manages the files, setup, and UI.
- **kmx:** The engine behind Bridge. [kmx](https://github.com/kaimahi-agents/kaimahi) already supports creating a
  local Orka cluster (`kmx cluster up`), running agent tests (`kmx package test`), and publishing and deploying agents
  (`kmx package deploy`). Bridge will run kmx as a sidecar and consume its JSON output rather than implementing its
  own Orka API or Kubernetes integration.
- **Local VM:** The Bridge app itself is small. On first run it downloads a VM image, published as a GitHub release
  asset, containing a container runtime, [kind](https://kind.sigs.k8s.io/), a preconfigured Orka cluster, and kmx.
  No separate installation is required. The planned runtimes are Apple's Virtualization framework on Apple silicon,
  WSL2 on Windows, and QEMU/KVM on Linux. The image and build pipeline are not implemented yet. For scale: Orka's
  own images plus the kind node image are about 800 MB compressed, before the VM's operating system.
- **Models:** Bridge uses your existing GitHub Copilot sign-in through a model gateway inside the VM. This route has
  been verified on a kind cluster: a native Orka agent fetched a real web page through Copilot with no API key and no
  model download. You can also provide an API key for OpenAI, Anthropic, or Azure OpenAI.

Two projects influenced the approach: [Hermes Agent](https://github.com/NousResearch/hermes-agent), for getting from
installation to a working agent without a terminal, and [Try Omarchy](https://github.com/omacom/try-omarchy), for
packaging a Linux VM inside a macOS app.

## What's next

The initial focus is the local development and deployment experience described above. Follow-up work includes:

- Scheduled agent runs
- Teams and Telegram messaging
- Skills and MCP tools in Designer
- Session history across clusters
- The VM image build, which the local VM choice needs, and its updates
- Shared team clusters
- Repository-based development with coding agents

Each feature has a separate issue so contributors can work independently. See the
[help wanted](https://github.com/orka-agents/orka-bridge/issues?q=is%3Aissue+is%3Aopen+label%3A%22help+wanted%22)
issues for opportunities to contribute. The initial screens will land through maintainer PRs. If you'd like to work on
one, comment on the relevant issue first to avoid duplicating work.

## Development

The native shell opens Overview, Designer and Clusters with empty screens.
It does not connect to kmx or a cluster yet.

Install Node 26.5.0, npm 11.17.0, Rust 1.98.1 and the
[Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/).
From `app/`:

```sh
npm ci
npm run lint
npm test
npm run build
npm run tauri dev
```

Build frontend assets before running the native checks:

```sh
cargo +1.98.1 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo +1.98.1 test --release --manifest-path src-tauri/Cargo.toml --locked --features custom-protocol
cargo +1.98.1 clippy --release --manifest-path src-tauri/Cargo.toml --locked --all-targets --features custom-protocol -- -D warnings
```

CI builds Windows x64 and Apple silicon Mac, with a native Mac navigation walk
and screenshots. These unsigned test artifacts are not installers or releases.
See [CONTRIBUTING.md](CONTRIBUTING.md) for platform build commands and notices.

## Contributing

This project welcomes contributions and suggestions.  Most contributions require you to agree to a
Contributor License Agreement (CLA) declaring that you have the right to, and actually do, grant us
the rights to use your contribution. For details, visit [Contributor License Agreements](https://cla.opensource.microsoft.com).

When you submit a pull request, a CLA bot will automatically determine whether you need to provide
a CLA and decorate the PR appropriately (e.g., status check, comment). Simply follow the instructions
provided by the bot. You will only need to do this once across all repos using our CLA.

This project has adopted the [Microsoft Open Source Code of Conduct](https://opensource.microsoft.com/codeofconduct/).
For more information see the [Code of Conduct FAQ](https://opensource.microsoft.com/codeofconduct/faq/) or
contact [opencode@microsoft.com](mailto:opencode@microsoft.com) with any additional questions or comments.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft
trademarks or logos is subject to and must follow
[Microsoft's Trademark & Brand Guidelines](https://www.microsoft.com/legal/intellectualproperty/trademarks/usage/general).
Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft sponsorship.
Any use of third-party trademarks or logos are subject to those third-party's policies.
