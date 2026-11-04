# Bridge happy path

## Why Bridge

Bridge gives you a controlled place on your own laptop to build and test agents for
[Orka](https://github.com/orka-agents/orka), an open-source platform that runs AI agents on Kubernetes clusters. Bridge
then sends the same agent to your team's cluster. It's for people who want to try an agent idea without first setting up
a cluster, installing Orka and connecting a model. With the local VM, the agent runs inside the VM, not directly on your
machine, and Bridge copies in only the agent and its test files. The agent is offered only the tools it declares, and
your model credentials stay out of agent files and packages.

**This is the planned path, not what the app does today.** Today Bridge has three mostly empty screens (Overview,
Designer and Clusters) and can't set up a VM, connect a model, run tests or deploy. The VM's building blocks and the
code that copies files in have landed here, but the app can't start the VM yet. kmx, the command-line tool that will do
the work behind the screens, can already create a local cluster, run tests and deploy on its own.

Each step says what the person does and what Bridge shows, then asks design questions under **Ideas wanted**.
The screenshots are design mockups; the names, sizes, times and token counts in them are made up.

## 1. Open Bridge

**You do:** Open Bridge for the first time. Clusters asks one question: start a local VM, or connect a cluster you
already have with Orka on it.

**Bridge shows:** One setup screen with progress. For the local VM, Bridge downloads a VM image that installs Orka on
its first start. If something is missing, it says what.

![Mockup: Clusters on first run, setting up the local VM and waiting for the model step](images/happy-path-setup.png)

**Ideas wanted:**

- How should the choice be worded for someone who doesn't know what a cluster is?
- The download is large: Orka's images and the Kubernetes node image are about 800 MB compressed, before the VM's
  operating system. How do we show the size and the wait?

## 2. Connect a model

**You do:** Sign in with GitHub Copilot, the default. Or enter an API key for OpenAI, Anthropic or Azure OpenAI.

**Bridge shows:** It runs one short test task through the model and shows Ready. The Copilot route has worked on a
test cluster outside the app, with no API key and no model download.

**Ideas wanted:**

- What should the sign-in flow look like, and where does it hand off to the browser?
- The proposal: when you reopen Bridge, Ready means a quick health check passed, and a fresh test task runs only when
  you ask. Is that clear, and how do we show when the last real test ran?
- Where should the API-key option sit so it's found but doesn't compete with sign-in?

## 3. Edit the example agent

**You do:** Open the Designer. An example agent is ready, with a prompt, a web tool and a test that only passes if the
agent really uses the tool. You change the prompt, choose tools, and add a Soul (the agent's standing persona and
tone, kept separate from its instructions) or Skills (reusable blocks of instructions shared between agents).

**Bridge shows:** The agent's sections, saved as Markdown files in a Git repository Bridge creates or one
you already have.

**Ideas wanted:**

- Form fields, the raw file, or both side by side?
- How do people pick tools: Orka's built-in tools and the custom tools on the target cluster in one list, or
  separated?
- Where do Soul and Skills belong, and how much explanation do they need on screen?

## 4. Run the tests

**You do:** Click Run.

**Bridge shows:** Pass or fail for each test, the tool calls the agent made, how long it took and how many tokens it
used.

![Mockup: the Designer after the first Run, with two tests passing, one failing and the tool calls](images/happy-path-designer.png)

**Ideas wanted:**

- When a test fails, what should the person see first: the check that failed, the agent's answer, or the tool calls?
- What counts as proof? A plausible answer isn't enough; the plan is to require evidence that the expected tool ran.
  How do we show that difference?

## 5. Send to the team's cluster

**You do:** Connect your team's cluster in Clusters if you haven't, choose it, and click Send.

**Bridge shows:** First, read-only checks: the cluster's Orka version, the model, and whether every tool the agent
uses is there. Then it runs the same tests on that cluster and deploys only if they pass. A missing custom tool
stops Send; Bridge doesn't copy or create it for you.

![Mockup: sending an agent to a team cluster, with the checks, the tests running there and Deploy waiting](images/happy-path-send.png)

**Ideas wanted:**

- A tool or permission is missing on the destination. What do we say, and what can the person do next?
- What should the deploy result show: where it runs, the test results there, a link to run it?
- How does a person tell a local pass from a pass on the team's cluster?

## 6. Overview

**You do:** Open Overview.

**Bridge shows:** Your agents grouped by where they run, the local VM and each connected cluster, with test status,
tools and recent runs.

![Mockup: Overview with agents grouped by the local VM and a team cluster](images/happy-path-overview.png)

**Ideas wanted:**

- What do you check day to day: did it run, did it fail, what did it cost?
- What should it show when a cluster can't be reached or its data is out of date?

## 7. Open questions

- **Windows with an existing cluster.** Without the VM, this needs kmx to run natively on Windows, which isn't
  finished. What do Windows users see meanwhile?
- **Updating the VM.** Each VM image will list its Orka and kmx versions, so Bridge can offer updates. When, and what
  happens to the old VM's agents and tests?
- **The second hour.** Running an agent on a schedule, and reaching it from Teams or Telegram. Teams needs a bot
  registration in your organization that Bridge can't do for you. Which comes first, and where does it live?
