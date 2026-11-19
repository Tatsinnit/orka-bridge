# Native Run: small correctness boundaries

Issue [#74](https://github.com/orka-agents/orka-bridge/issues/74) extends the
working native Run path with three independently testable corrections. Start
with the existing parser, runner and transport; grow their contracts only where
the observed edge cases require it. This follows Gall's law: evolve the small
working flow rather than introduce another execution framework.

## Separation of concerns

| Boundary | Responsibility |
|---|---|
| `engine/run_source.rs` | Decode immutable saved bytes, validate the supported source schema, classify builtin names, extract and validate all test cases before preparation. No I/O. |
| `engine/run.rs` | Prepare resource bodies, submit once, retain acknowledged identities and uncertain writes, observe only acknowledged resources, aggregate evidence. |
| `RunTransport` implementations | Native-owned endpoint, namespace, credentials, request bounds and connection lifetime. Source files cannot select transport authority. |
| Existing app owners and renderer | Preserve the run receipt and present the report. An evidence verdict is separate from submission/observation state. |

## Test fences: explicit support, no silent omission

Native Run accepts a top-level opening line exactly <code>```yaml agent-test</code>.
Each block decodes as one case using the existing closed YAML schema. Closing
fences are exactly three backticks at the top level. Indented backticks inside
YAML block scalars remain test input. A missing test closing fence refuses the
source.

Indented, tilde, longer-opening and other noncanonical `agent-test` fences are
rejected before any Agent or Task is submitted, including when a valid case
appears elsewhere in the file. This is intentionally a small supported source
format, not a second general Markdown implementation. Authors can move the test
to the canonical fence without changing its YAML.

Top-level ordinary Markdown code examples are tracked by delimiter character
and opening length. An apparent test inside an example is inert; a shorter or
different delimiter does not close that example. Backtick spans on one line and
list-indented examples are prose for this deliberately narrow scanner, so their
closers cannot hide a later test. Fence recognition and case YAML decoding are
separate helpers so Markdown delimiters cannot become execution policy. Unclosed
top-level example fences also refuse the source rather than hiding later tests.

## Builtin names: classification before mutation

`kind: builtin` must name an exact known builtin, not merely a nonempty string.
Unknown, differently cased or padded names are source errors, so they cannot
fall through to an otherwise authorized custom Tool when an Agent is created.

The explicit native catalogue follows `KnownBuiltInToolNames` (default registry,
chat and coordination names) at Orka source
[`3cb1f0df`](https://github.com/orka-agents/orka/blob/3cb1f0df36705af9ff81cb8b4e8a0f8d36ee6eea/internal/tools/registry.go),
with coordination names in
[`internal/aitools/resolver.go`](https://github.com/orka-agents/orka/blob/3cb1f0df36705af9ff81cb8b4e8a0f8d36ee6eea/internal/aitools/resolver.go).
Review this catalogue when changing supported Orka versions. New builtins remain
unsupported until reviewed. A declaration using `kind: tool` remains a distinct
custom Tool resource and is checked through the existing target catalogue path.

This check classifies source intent, not target availability or authorization.
Orka and the existing connection/permission checks still decide what the target
can execute. No new tool-discovery request, guest protocol operation or
credential path is needed for this correction.

## Partial runs: failure is evidence, missing is unknown

Submission may acknowledge an Agent and only some of its Tasks. Observation
reads those receipts only; an uncertain write is never retried or looked up by
an unacknowledged name. When a caller explicitly observes the retained handle
on a still-usable transport, the native report includes each missing case as
`Unacknowledged`, with no answer/tool evidence and unknown assertions/verdict.
That phase does not assert that the remote Task is absent or cancelled. An app
owner whose transport was retired by the submission failure keeps its existing
unknown submit error and no report rather than inventing observations.

Use the existing aggregation rule for the complete case list:

- An observed cancellation retains the existing cancelled verdict.
- Otherwise, any known failed case makes the run verdict failed.
- Otherwise, any unknown case makes the run verdict unknown.
- Passed requires every case to be passed.

An incomplete submission therefore cannot erase an observed failure or become
a pass. Transport errors and Stop retain their existing handling; this does
not add automatic observation recovery or remote cancellation.

## Verification

`app/src-tauri/tests/native_run.rs` covers noncanonical fences beside valid
cases, nested Markdown examples, inline spans, list-indented examples, YAML
literal backticks, exact builtin names, and a deterministic transport that loses
the second Task reply. The latter observes the retained first Task across
failed, cancelled, assertion-failed, passed and missing-answer outcomes, and
checks that observation performs only reads of acknowledged resources. These
are native runner tests, not evidence of a live model/VM run.
`tests/example_run.rs` additionally exercises the saved-file app owner through
the VM protocol fixture: an unknown builtin refuses before any remote write and
leaves the client usable.

No live-model/VM or native UI walk was performed for these edge cases. Report
presentation continues through the existing renderer; the new missing-case
phase is verified at the native report boundary.
