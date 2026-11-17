---
schemaVersion: 1
---

# web-reader tests

This test passes only if the agent fetches the page. The page is the SHA-256
checksum Kubernetes publishes for the kubectl v1.35.0 download for Apple silicon
Macs. It contains only that checksum and doesn't change after the release, so
the check compares the answer with the whole page. A model can't guess the 64
hexadecimal characters and is very unlikely to recall them, and the agent's
prompt doesn't contain them. The agent has no web_search tool, which can return
mock results when its search backend fails.

```yaml agent-test
id: published-checksum
title: "Copies the checksum from the page"
input: What SHA-256 checksum does https://dl.k8s.io/release/v1.35.0/bin/darwin/arm64/kubectl.sha256 publish for kubectl v1.35.0 on Apple silicon Macs?
assertions:
  - id: checksum-from-page
    type: contains
    value: cf699c56340dc775230fde4ef84237d27563ea6ef52164c7d078072b586c3918
  - id: fetched-page
    type: toolCalled
    tool: web_fetch
```
