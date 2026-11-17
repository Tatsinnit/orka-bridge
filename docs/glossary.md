# Bridge glossary

Use these words in app headings, buttons, help and status messages. Give each idea one name. Bold phrases below are exact UI labels where indicated.

| Term | One meaning | Replaces or clarifies |
|---|---|---|
| **cluster** | A Kubernetes cluster with Orka installed, where agents run. It can be the local cluster or a cluster you already have. | place, workspace, environment, an “Orka” used to mean a destination |
| **local cluster** | The cluster Bridge sets up and runs on this computer. | local workspace, workspace, local Orka, local used alone as a destination |
| **this computer** | The Mac or PC running Bridge, not the cluster itself. | host, machine, laptop when either supported device is meant |
| **AI** | The model service an agent connects to for answers. | Say **Connect the AI** for the action and **AI connection** for a configured route to that service. This replaces gateway, model route and Provider (Orka's route configuration, not the model company). |
| **model** | A specific model the agent uses, such as the model selected in Designer. | Not a synonym for AI connection or sign-in. Keep actual model names unchanged. |
| **agent** | An assistant defined by its instructions, model and tools. | Lowercase in sentences, not Agent or Orka Agent. Say **agent on the cluster** when distinguishing it from its file. |
| **test** | A task for an agent, with checks that decide whether it passed. | Test Task; a task that finishes but fails its checks is a failed test. |
| **tool** | An action an agent can use, such as fetching a web page. | Tool, workspace tool; keep actual tool names unchanged |
| **agent file** | The editable file that defines an agent. | agent definition, source artifact; use **test file** for the separate file containing tests |
| **Send** | The action that creates or updates the agent on the selected cluster and runs its tests there. | deploy, publish; do not imply a failed test removes the agent |
| **sign-in** | The saved account access used to connect the AI. | credentials or token in everyday copy; it is not the AI connection itself |
| **context** | A named Kubernetes connection from the user's kubeconfig file. | Keep **context** in the cluster connection form; it is not another word for cluster. |

## Use the words together

- **Clusters** is the tab and page heading. Use **Choose where agents run** as its subtitle, **Add a cluster**, **No cluster selected** and **Back to clusters**.
- Use **This computer** as the device's label in the cluster list, and “this computer” in sentences; call the installed result **the local cluster**. For example: “Set up the local cluster on this computer.”
- Name the setup step **Run your first agent**. Its result says **Its test passed** or **Its test failed**, with **See what it answered** and **Run it again**. Keep **Tests** and **Run tests** in Designer.
- Say **Connect the AI**, **AI connected** or **AI needs attention**. Do not repeat the same state within one message.
- Lead with the result and the next action. Cut storage and implementation explanations from everyday copy. Keep troubleshooting facts in Details.
- Orka remains the platform's proper name, not a synonym for cluster. Keep literal file contents, identifiers, paths, resource types (including Agent and Provider), and diagnostic output accurate in File view and Details. Words such as token and credentials belong there when needed for troubleshooting. Do not rename API fields or saved data. Outside those views, use plain descriptions such as **agent without a linked file**, not **Unassociated Agent**.
