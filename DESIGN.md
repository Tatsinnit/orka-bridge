# Bridge design

How Bridge looks and behaves on screen. Every screen uses these rules. If a screen seems to need something new, change
this file first, in its own pull request.

The palette and type come from the Orka website; the tab bar, window and two-column layouts come from the approved
frames. This file extends the existing tokens in [`app/src/app.css`](app/src/app.css): `--font-*`, `--type-*`,
`--size-*`, `--space-*`, `--radius-*` and `--color-*`. Use the
[Bridge glossary](https://github.com/orka-agents/orka-bridge/blob/2df7dba2a3c0e751271b25a15b9850221e320abe/docs/glossary.md)
for UI concepts and labels.

In these rules, ink, ink-2 and ink-3 mean `--color-text`, `--color-text-secondary` and `--color-text-tertiary`;
line-2 means `--color-border-strong`; card and well mean `--color-card` and `--color-chrome`. Inverse uses
`--color-action` and `--color-action-text`. These are design rules, not a claim that every screen already follows them.

## Type

Geist for everything people read. Geist Mono only for names the creator chose (agent, test, tool, model, cluster,
file path), commands and log lines, and generated values in Details. Never for prose. Ligatures off.

| Step | Size / line · weight | Used for |
|---|---|---|
| Title | 21/26 · 600 | Page heading, one per screen |
| Card title | 15/20 · 600 | Card and result heads |
| Reading | 15/24 · 400 | The prompt, a test's question, the agent's answer |
| Body | 13/18 · 400, 500 | Everything else, including help text and status sentences |
| Small | 11.5/16 · 400, 500 | Times, counts, pills, tab subtitles, the status bar |
| Section label | 11.5/16 · 600, uppercase | Section names inside long cards only |

- Four sizes. If something seems to need a fifth, use weight, ink-2 or a section label instead.
- Small never carries a sentence the creator has to read. A status sentence is Body.
- Inline mono uses `0.92em` and never a smaller size mid-line.

## Spacing and radii

- Spacing steps: 4, 8, 12, 16, 24, 32. Nothing else.
- 24 px page margins. Everything starts on the left margin; no centred columns under a left heading.
- One 6 px radius for controls, chips, pills, rows, callouts and cards. Circles and status dots are geometric icons,
  not another container radius.
- Rows inside a card stay inside the card's 16 px padding, borders included.

## Controls

One height for everything you click or type in, so a row of mixed controls lines up.

| Size | Height | Where |
|---|---|---|
| Default | 32 px, 12 px side padding | Every button, input and select on a page or in a card |
| Small | 26 px, 10 px side padding | Actions inside list rows, segmented controls, pills that act |

Button kinds:

| Kind | Look | Use |
|---|---|---|
| Primary | Inverse ink fill and inverse text | The one next step. At most one per card, and one per page head |
| Secondary | Card fill, line-2 ring, ink text, same in light and dark | Other real actions |
| Quiet | No box, ink-2, underline on hover | Back, Change, Remove, Show me how, help links |
| Icon | 26 px square, no box until hover, with a label for screen readers | Close (X), Copy |

- Disabled: well fill, no ring, ink-3. A disabled control says why next to it or in its tooltip. A status is never a
  disabled button.
- Destructive actions are secondary or quiet in ink, and always confirm first. Red reports a failed state; it does not
  decorate an action.
- A card holds one primary, at most two secondaries and any quiet actions. More than that goes behind Details or into
  the screen the action belongs to.
- Actions sit in one row, in this order: primary, secondaries, quiet. They sit at the bottom of the card they act on,
  or at the right of its head for long cards. Never outside the card.

Fields:

- Inputs and selects have card fill and a line-2 ring in both themes. Placeholder text uses ink-2. The page tone is
  for read-only code only.
- Selects are drawn by Bridge (chevron at the right), never the system control, so they match the theme.
- Radios and checkboxes use ink, not the operating system accent colour. Links use ink and an underline.
- A form is one column 520 px wide. Fields, help text and errors share that width. A short field (a name, a model)
  may be narrower, never wider.
- Help text sits under its field in ink-2. An error replaces it in the failed colour, above the action that retries.

Choices:

- Two to four exclusive views (Agent, Chat, File; By cluster, By agent) are a segmented control: a well track, the
  chosen one on card fill with a line-2 edge. Not a row of secondary buttons.
- Pickers open as a popover over the page. They close with the X at the top right, Escape, or a click outside.

## Disclosure

One component for anything that folds: a chevron, then the label in Body 500 ink-2, then an optional hint in ink-3.
The chevron points right when closed and down when open. It is a real button (or `summary`) with `aria-expanded`.

- Labels are sentence case: "Details", "More settings", "Show details". Never uppercase, never ▶ or ›.
- What opens appears directly under the control, before anything that follows it.
- Generated ids, hashes, the agent and test files and logs live behind a disclosure, closed by default.

## Status

Six statuses. Every state word in the app maps to one of them, so the same icon always means the same thing.

| Status | Icon | Colour | Words that map here |
|---|---|---|---|
| Passed | circle with a check | green | Passed, Ready, Connected |
| Failed | circle with a cross | red | Failed, Couldn't start, Denied |
| Running | half-filled circle | blue | Installing, Running (a test), Checking |
| Stale | dashed ring | slate | Not run since your edit, Earlier files, Result unknown |
| Needs you | ring with a dot | amber | Needs you, Couldn't check |
| Not started | plain ring | ink-3 | Not started, Waiting, Not checked, Pick any time |

- A pill holds one state word. Times, counts and roles go in the line next to it, never in the pill.
- Colour and tint appear on the pill and on each row's icon. Row text stays ink.
- Amber only means the app is waiting on the creator. Busy is blue. Failed is never green.
- A cluster that is up and usable is Ready (green), not Running.

The colour values are the tokens `--color-status-*` and `--color-tint-*`.

## Layout

- Two-column pages have one stack per column. A card's height never moves a card in the other column.
- The main column is on the left (1.15fr), the side column on the right (1fr), 24 px apart.
- List rows are a grid on fixed columns: icon, name with one meta line, status pill, one small action. Names line up
  row to row.
- Long cards (a run result, a Send, setup) keep this order: head with title, one pill and the next action; one summary
  line; What you send (Send and setup only); What happened, failures first; Details, folded.
- One scroll area per page.

## Compact (under 820 px)

- One column. The card that holds the next action comes first, then status and progress, then history and details.
  On the wide layout the same cards keep their columns.
- Tab subtitles hide. The cluster button keeps its dot and shortens the name with an ellipsis at 160 px.
- A list row becomes two lines: icon and name with meta; then the pill at the left and the action at the right, both
  under the name.

## Focus

- Every control shows a 2 px ink ring, 2 px out, when focused from the keyboard. Never remove it.
- A heading that takes focus after navigation (so screen readers announce the page) shows no ring.
- Focus moves to the page heading on navigation, and back to the control that opened a popover when it closes.

## Ids and names

- Names the creator chose show in full, in mono, at the line's size.
- Generated values (digests, hashes, generated names) appear only in Details or a log, shortened to kind, first four
  and last four (`sha256:7c17…a370`), with Copy for the whole value. Never in a heading or a sentence.

## Light and dark

The theme follows the system. Both themes use the same rules; only the token values change. Paper, surface, soft,
line, ink and inverse values match the Orka website. Secondary buttons and fields use card fill and a line-2 edge.

- Colour appears only when it communicates status: amber needs you, blue running, green passed and red failed. The
  label or icon shape must still communicate the state without colour.
- All text, including status text on its tint, meets WCAG AA contrast of at least 4.5:1. Decorative separators are not
  text or control boundaries.
