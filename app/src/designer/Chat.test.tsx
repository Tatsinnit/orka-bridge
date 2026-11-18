import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Chat from './Chat';
import { Profiler } from 'react';
import { readFileSync } from 'node:fs';
import type { ChatSnapshot, ChatTurn } from './chat-api';
const id = '9b2b93c4-375c-4815-9717-39852a96e411';
const idle: ChatSnapshot = { documentId: id, state: 'ready', busy: false, canSend: true, versions: [], error: null };
const turn: ChatTurn = { message: 'Hello', answer: 'Hello back', phase: 'Succeeded', tools: [], contextRetained: true, terminationUnconfirmed: false, stopUnconfirmed: false };
let snapshot: ChatSnapshot; let calls: { command: string; args: Record<string, unknown> }[];
let send: (() => unknown) | undefined;
beforeEach(() => {
  vi.useFakeTimers(); vi.stubGlobal('isTauri', true); snapshot = structuredClone(idle); calls = []; send = undefined;
  mockIPC((command, args) => {
    calls.push({ command, args: args as Record<string, unknown> });
    if (command === 'agent_chat_send') return send ? send() : snapshot;
    if (command === 'agent_chat_new') snapshot = { ...snapshot, state: 'ready', canSend: true, error: null };
    return structuredClone(snapshot);
  });
});
afterEach(() => { cleanup(); clearMocks(); vi.useRealTimers(); vi.unstubAllGlobals(); });
async function open(blocked?: string) { const result = render(<Chat documentId={id} blocked={blocked} />); await act(async () => {}); return result; }
async function poll() { await act(async () => { await vi.advanceTimersByTimeAsync(500); }); }
function field() { return screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement; }
function enter(value = 'Hello') { fireEvent.change(field(), { target: { value } }); field().focus(); fireEvent.keyDown(field(), { key: 'Enter' }); }
const sends = () => calls.filter(c => c.command === 'agent_chat_send');
test('Chat names this computer, explains test independence and numbers only multiple conversations', async () => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  await open();
  expect(screen.getByRole('region', { name: 'Chat on this computer' })).toBeTruthy();
  expect(screen.getByRole('heading', { name: 'Chat on this computer' })).toBeTruthy();
  expect(screen.queryByText('Conversation 1')).toBeNull();
  expect(screen.queryByRole('region', { name: 'Conversation 1' })).toBeNull();
  snapshot = { ...idle, versions: [{ turns: [turn] }, { turns: [] }] }; await poll();
  expect(screen.getByRole('heading', { name: 'Conversation 1' })).toBeTruthy();
  expect(screen.getByRole('heading', { name: 'Conversation 2' })).toBeTruthy();
});
test('empty Chat is a cardless conversation with only its labelled composer, without routine status clutter', async () => {
  await open();
  const panel = screen.getByRole('region', { name: 'Chat on this computer' });
  expect(panel.classList.contains('card')).toBe(false);
  expect(screen.queryByText(/Ask your agent a question/)).toBeNull();
  expect(screen.queryByText('Ready for your message.')).toBeNull();
  expect(screen.queryByRole('button', { name: 'New conversation' })).toBeNull();
  expect(screen.getByText('Enter sends · Shift+Enter for a new line')).toBeTruthy();
  const send = screen.getByRole('button', { name: 'Send message' });
  expect(send.textContent).toBe('Send');
  expect(send.parentElement).toBe(field().parentElement);
  expect(send.hasAttribute('disabled')).toBe(true);
});
test('a conversation discloses bounded, memory-only history', async () => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] }; await open();
  fireEvent.click(screen.getByText('About this conversation'));
  expect(screen.getByText(/50 messages/)).toBeTruthy(); expect(screen.queryByText(/new worker/)).toBeNull();
  expect(screen.getByText(/quit Bridge/)).toBeTruthy(); expect(screen.getByText(/remain in the workspace/)).toBeTruthy();
});
test('Enter sends exactly once and only an appended turn consumes its matching composer', async () => {
  send = () => (snapshot = { ...idle, state: 'preparing', busy: true, canSend: false });
  await open(); enter(); fireEvent.keyDown(field(), { key: 'Enter' }); await act(async () => {});
  expect(sends()).toEqual([{ command: 'agent_chat_send', args: { documentId: id, message: 'Hello' } }]);
  expect(field().value).toBe('Hello'); expect(document.activeElement).toBe(field());
  snapshot = { ...idle, versions: [{ turns: [turn] }] }; await poll();
  expect(field().value).toBe(''); expect(screen.getByText('Hello back')).toBeTruthy();
});
test('a newer composer draft survives acceptance of the earlier message', async () => {
  send = () => (snapshot = { ...idle, state: 'preparing', busy: true, canSend: false });
  await open(); enter(); await act(async () => {});
  fireEvent.change(field(), { target: { value: 'Next question' } });
  snapshot = { ...idle, versions: [{ turns: [turn] }] }; await poll(); expect(field().value).toBe('Next question');
});
test.each(['busy', 'source', 'model'])('native %s rejection keeps the message and permits retry', async error => {
  send = () => Promise.reject(error); await open(); enter(); await act(async () => {});
  expect(field().value).toBe('Hello'); expect(screen.getByRole('alert')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(false);
});
test('an uncertain Send reply retains acknowledgement and cannot replay before status shows its turn', async () => {
  send = () => Promise.reject('unknown transport error'); await open(); enter(); await act(async () => {});
  expect(screen.getByRole('alert').closest('form')).toBe(field().closest('form'));
  expect(document.querySelector('.chat-status')?.closest('form')).toBe(field().closest('form'));
  await poll(); expect(field().value).toBe('Hello');
  expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(true);
  expect(screen.getByRole('button', { name: 'New conversation' }).hasAttribute('disabled')).toBe(true);
  fireEvent.keyDown(field(), { key: 'Enter' }); await act(async () => {}); expect(sends()).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: 'Reconnect' })); await act(async () => {});
  expect(calls.filter(c => c.command === 'agent_chat_reconnect')).toHaveLength(0);
  snapshot = { ...idle, versions: [{ turns: [turn] }] }; await poll();
  expect(field().value).toBe(''); expect(screen.getByText('Hello back')).toBeTruthy(); expect(sends()).toHaveLength(1);
  expect(screen.queryByRole('alert')).toBeNull();
});
test('terminal unavailable history releases uncertain admission for New conversation without losing the draft', async () => {
  send = () => Promise.reject('unknown transport error'); await open(); enter(); await act(async () => {});
  snapshot = { ...idle, state: 'contextUnavailable', canSend: false, error: 'response' }; await poll();
  expect(field().value).toBe('Hello');
  expect(screen.getByRole('button', { name: 'New conversation' }).hasAttribute('disabled')).toBe(false);
  expect(screen.getByRole('alert').textContent).toContain('New conversation');
  expect(screen.getByRole('alert').textContent).not.toContain('Reconnect');
});
test('automatic acknowledgement clears only the uncertain Send warning', async () => {
  send = () => Promise.reject('unknown transport error'); await open(); enter(); await act(async () => {});
  expect(screen.getByRole('alert')).toBeTruthy();
  snapshot = { ...idle, versions: [{ turns: [turn] }] }; await poll();
  expect(field().value).toBe(''); expect(screen.queryByRole('alert')).toBeNull();
});
test('an uncertain Send can reconcile an observed preflight without a turn while retaining the message', async () => {
  send = () => Promise.reject('unknown transport error'); await open(); enter(); await act(async () => {});
  snapshot = { ...idle, state: 'preparing', busy: true, canSend: false }; await poll();
  snapshot = { ...idle, error: 'model' }; await poll();
  expect(field().value).toBe('Hello'); expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(false);
});
test('a pre-turn observation loss retains acknowledgement through native reconnect', async () => {
  send = () => Promise.reject('unknown transport error'); await open(); enter(); await act(async () => {});
  snapshot = { ...idle, state: 'preparing', busy: true, canSend: false }; await poll();
  snapshot = { ...idle, state: 'observationLost', busy: false, canSend: false }; await poll();
  fireEvent.click(screen.getByRole('button', { name: 'Reconnect' })); await act(async () => {});
  expect(calls.filter(c => c.command === 'agent_chat_reconnect')).toHaveLength(1);
  snapshot = { ...idle, versions: [{ turns: [turn] }] }; await poll(); expect(field().value).toBe('');
});
test('ready-state connection preflight failure offers safe retry, not an impossible reconnect', async () => {
  snapshot = { ...idle, error: 'connection' }; await open();
  expect(screen.queryByText(/do not resend/i)).toBeNull();
  expect(screen.getByRole('alert').textContent).toContain('try sending again');
  expect(screen.queryByRole('button', { name: 'Reconnect' })).toBeNull();
});
test('background status polling does not immediately erase an actionable admission error', async () => {
  send = () => Promise.reject('busy'); await open(); enter(); await act(async () => {});
  await poll(); expect(screen.getByRole('alert').textContent).toContain('busy'); expect(field().value).toBe('Hello');
});
test('asynchronous preflight failure before a turn keeps the message', async () => {
  send = () => (snapshot = { ...idle, state: 'preparing', busy: true, canSend: false });
  await open(); enter(); await act(async () => {});
  snapshot = { ...idle, error: 'model' }; await poll();
  expect(field().value).toBe('Hello'); expect(screen.getByText(/Connect the AI/)).toBeTruthy();
  expect(sends()).toHaveLength(1);
});
test('identical earlier text is not mistaken for the newly admitted turn', async () => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  send = () => (snapshot = { ...snapshot, state: 'preparing', busy: true, canSend: false });
  await open(); enter(); await act(async () => {}); await poll(); expect(field().value).toBe('Hello');
  snapshot = { ...idle, versions: [{ turns: [turn, turn] }] }; await poll(); expect(field().value).toBe('');
});
test('Shift Enter and IME composition do not submit or prevent normal editing', async () => {
  await open(); fireEvent.change(field(), { target: { value: 'Lines' } });
  expect(fireEvent.keyDown(field(), { key: 'Enter', shiftKey: true })).toBe(true);
  fireEvent.compositionStart(field()); fireEvent.keyDown(field(), { key: 'Enter' }); fireEvent.compositionEnd(field());
  fireEvent.keyDown(field(), { key: 'Enter', isComposing: true });
  fireEvent.keyDown(field(), { key: 'Enter', keyCode: 229 }); await act(async () => {}); expect(sends()).toEqual([]);
});
test('blocked save/setup/remote reasons prevent IPC while retaining the composer', async () => {
  const view = await open('Wait for your changes to save.'); enter(); await act(async () => {});
  expect(sends()).toEqual([]); expect(field().value).toBe('Hello');
  expect(screen.getByText('Wait for your changes to save.')).toBeTruthy();
  view.rerender(<Chat documentId={id} />); await act(async () => {});
  fireEvent.keyDown(field(), { key: 'Enter' }); await act(async () => {}); expect(sends()).toHaveLength(1);
});
test('remount restores conversation by status without replaying Send', async () => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  const view = await open(); view.unmount(); await open();
  expect(screen.getByText('Hello back')).toBeTruthy(); expect(sends()).toEqual([]);
});
test('renders labelled versions and literal untrusted text, with tools behind Details', async () => {
  snapshot = { ...idle, versions: [{ turns: [turn] }, { turns: [{ ...turn, message: '<img src=x onerror=alert(1)>', answer: '<script>secret()</script>', tools: [{ name: '<iframe>', callId: 'private-id', phase: 'Succeeded' }] }] }] };
  const view = await open(); expect(screen.getByRole('heading', { name: 'Conversation 2' })).toBeTruthy();
  expect(screen.getByText('<script>secret()</script>')).toBeTruthy(); expect(view.container.querySelector('img,script,iframe')).toBeNull();
  expect(screen.queryByText('private-id')).toBeNull();
  const tools = screen.getByRole('list', { name: 'Tool activity' });
  expect(tools.closest('details')).toBeNull();
  expect(within(tools).getByText('<iframe>')).toBeTruthy();
});
test('read-only reconnect never resends an observation-lost message', async () => {
  snapshot = { ...idle, state: 'observationLost', canSend: false, versions: [{ turns: [{ ...turn, answer: null }] }] };
  await open(); fireEvent.click(screen.getByRole('button', { name: 'Reconnect' })); await act(async () => {});
  expect(calls.some(c => c.command === 'agent_chat_reconnect')).toBe(true); expect(sends()).toEqual([]);
  expect(screen.getByText(/connection was lost/i)).toBeTruthy();
  expect(screen.getByRole('button', { name: 'New conversation' }).hasAttribute('disabled')).toBe(true);
});
test.each([
  ['requestingStop', 'Requesting stop…'],
  ['stopRequested', 'Stop requested. Execution termination is unconfirmed.'],
  ['stopUnconfirmed', 'Stop request unconfirmed. Execution may still be running.'],
] as const)('shows %s distinctly, never a Stopped guarantee', async (state, text) => {
  snapshot = { ...idle, state, canSend: false }; await open();
  expect(screen.getByText(text)).toBeTruthy(); expect(screen.queryByText(/^Stopped$/)).toBeNull();
});
test('Stop dispatches only stop and its historical notice remains after Ready and a following turn', async () => {
  snapshot = { ...idle, state: 'working', busy: true, canSend: false, versions: [{ turns: [{ ...turn, answer: null }] }] };
  await open(); fireEvent.click(screen.getByRole('button', { name: 'Stop' })); await act(async () => {});
  expect(calls.some(c => c.command === 'agent_chat_stop')).toBe(true); expect(sends()).toEqual([]);
  snapshot = { ...idle, versions: [{ turns: [{ ...turn, terminationUnconfirmed: true, contextRetained: false }, { ...turn, message: 'After Stop' }] }] };
  await poll(); expect(screen.getByText('Stop requested. Execution termination is unconfirmed.')).toBeTruthy();
  expect(screen.getByText(/not confirmed in later context/)).toBeTruthy();
});
test.each(['Pending', 'Queued', 'Scheduled', ''])('a working snapshot with phase %s shows waiting to start', async phase => {
  snapshot = { ...idle, state: 'working', busy: true, canSend: false, versions: [{ turns: [{ ...turn, answer: null, phase }] }] };
  await open(); expect(screen.getByText('Waiting to start…')).toBeTruthy();
});
test.each(['Pending', 'Queued', 'Scheduled', 'Running', 'Finalizing'])('pending %s reply reads as waiting, not missing history', async phase => {
  snapshot = { ...idle, state: 'working', busy: true, canSend: false, versions: [{ turns: [{ ...turn, answer: null, contextRetained: false, phase }] }] };
  await open();
  const conversation = screen.getByRole('region', { name: 'Conversation' });
  expect(within(conversation).queryAllByText(/No answer (observed|was received)|not confirmed in later context/)).toHaveLength(0);
  expect(within(conversation).getByText(['Running', 'Finalizing'].includes(phase) ? 'Waiting for your agent’s reply…' : 'Waiting for your agent to start…')).toBeTruthy();
  expect(conversation.querySelector('.status-running')).toBeTruthy();
});
test('a newly admitted turn stays waiting while the saved agent is preparing', async () => {
  snapshot = { ...idle, state: 'preparing', busy: true, canSend: false, versions: [{ turns: [{ ...turn, answer: null, contextRetained: false, phase: '' }] }] };
  await open();
  const conversation = screen.getByRole('region', { name: 'Conversation' });
  expect(within(conversation).queryAllByText(/No answer was received|not confirmed in later context/)).toHaveLength(0);
  expect(within(conversation).getByText('Waiting for your agent to start…')).toBeTruthy();
});
test.each([
  ['waitingForSession', null], ['waitingForSession', 'An already observed answer'],
  ['working', null], ['working', 'An already observed answer'],
] as const)('%s confirmation keeps answer %s and postpones context claims', async (state, answer) => {
  snapshot = { ...idle, state, busy: true, canSend: false, versions: [{ turns: [
    { ...turn, message: 'Earlier question', contextRetained: false },
    { ...turn, answer, contextRetained: false },
  ] }] };
  await open();
  const conversation = screen.getByRole('region', { name: 'Conversation' });
  expect(within(conversation).queryByText('No answer was received.')).toBeNull();
  expect(within(conversation).getAllByText(/not confirmed in later context/)).toHaveLength(1);
  if (answer) expect(within(conversation).getByText(answer)).toBeTruthy();
  else expect(within(conversation).getByText(state === 'waitingForSession' ? 'Waiting for the conversation to finish saving…' : 'Confirming your agent’s reply…')).toBeTruthy();
  snapshot = { ...snapshot, state: 'ready', busy: false, canSend: true };
  await poll();
  expect(within(conversation).getAllByText(/not confirmed in later context/)).toHaveLength(2);
  if (!answer) expect(within(conversation).getByText('No answer was received.')).toBeTruthy();
});
test.each(['Failed', 'Cancelled'])('Session confirmation preserves %s and stop warnings', async phase => {
  snapshot = { ...idle, state: 'waitingForSession', busy: true, canSend: false, versions: [{ turns: [
    { ...turn, answer: null, phase, contextRetained: false, terminationUnconfirmed: true },
  ] }] };
  await open();
  expect(screen.getByText(phase === 'Failed' ? 'This turn failed.' : /The cluster reported this turn cancelled/)).toBeTruthy();
  expect(screen.getByText('Stop requested. Execution termination is unconfirmed.')).toBeTruthy();
  expect(screen.queryByText(/not confirmed in later context|failed without an answer|No answer was received/)).toBeNull();
});
test('terminal unconfirmed history remains distinct from an in-progress reply', async () => {
  snapshot = { ...idle, versions: [{ turns: [{ ...turn, contextRetained: false }] }] };
  await open(); expect(screen.getByText(/not confirmed in later context/)).toBeTruthy();
});
test('composer grows with text and shrinks again after acceptance', async () => {
  await open();
  Object.defineProperty(field(), 'scrollHeight', { configurable: true, value: 144 });
  fireEvent.change(field(), { target: { value: 'A long message' } });
  expect(field().style.height).toBe('144px');
  snapshot = { ...idle, versions: [{ turns: [{ ...turn, message: 'A long message' }] }] };
  enter('A long message'); await act(async () => {});
  expect(field().value).toBe('');
  expect(field().style.height).toBe('auto');
});
test('composer remeasures an unchanged draft when window wrapping changes', async () => {
  await open();
  let height = 80;
  Object.defineProperty(field(), 'scrollHeight', { configurable: true, get: () => height });
  fireEvent.change(field(), { target: { value: 'A draft that wraps differently after resizing' } });
  expect(field().style.height).toBe('80px');
  height = 180; fireEvent(window, new Event('resize'));
  expect(field().style.height).toBe('180px');
  height = 80; fireEvent(window, new Event('resize'));
  expect(field().style.height).toBe('80px');
});
test('composer waits for activation to remeasure and removes its resize listener on unmount', async () => {
  const view = await open();
  let height = 80; const measure = vi.fn(() => height);
  Object.defineProperty(field(), 'scrollHeight', { configurable: true, get: measure });
  fireEvent.change(field(), { target: { value: 'Keep this draft' } });
  view.rerender(<Chat documentId={id} active={false} />); measure.mockClear();
  height = 180; fireEvent(window, new Event('resize'));
  expect(measure).not.toHaveBeenCalled();
  view.rerender(<Chat documentId={id} active />);
  expect(field().style.height).toBe('180px');
  view.unmount(); measure.mockClear(); fireEvent(window, new Event('resize'));
  expect(measure).not.toHaveBeenCalled();
});
test('a retained historical Finalizing turn does not look like an active reply', async () => {
  snapshot = { ...idle, versions: [{ turns: [{ ...turn, phase: 'Finalizing', answer: null, contextRetained: false }] }] };
  await open();
  expect(screen.getByText('No answer was received.')).toBeTruthy();
  expect(screen.getByText(/not confirmed in later context/)).toBeTruthy();
});
test('a failed turn with an observed answer does not look like a successful turn', async () => {
  snapshot = { ...idle, versions: [{ turns: [{ ...turn, phase: 'Failed' }] }] };
  await open(); expect(screen.getByText('This turn failed.')).toBeTruthy(); expect(screen.getByText('Hello back')).toBeTruthy();
});
test('an uncertain Stop IPC reply does not forget an admitted message awaiting its turn', async () => {
  mockIPC(command => {
    calls.push({ command, args: {} });
    if (command === 'agent_chat_stop') return Promise.reject('unknown transport error');
    return snapshot;
  });
  send = undefined; snapshot = { ...idle }; await open();
  snapshot = { ...idle, state: 'preparing', busy: true, canSend: false };
  enter(); await act(async () => {}); expect(field().value).toBe('Hello');
  fireEvent.click(screen.getByRole('button', { name: 'Stop' })); await act(async () => {});
  snapshot = { ...idle, versions: [{ turns: [turn] }] }; await poll();
  expect(field().value).toBe(''); expect(screen.getByText('Hello back')).toBeTruthy();
});
test('unsupported input requests keep Stop without inventing approvals', async () => {
  snapshot = { ...idle, state: 'waitingForInput', busy: true, canSend: false }; await open();
  expect(screen.getByText(/input that Chat does not support/)).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Stop' })).toBeTruthy(); expect(screen.queryByRole('button', { name: /approve/i })).toBeNull();
});
test('New conversation retains earlier transcript and adds no message', async () => {
  snapshot = { ...idle, state: 'contextUnavailable', canSend: false, versions: [{ turns: [turn] }] };
  await open(); expect(screen.queryByRole('button', { name: 'Reconnect' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'New conversation' })); await act(async () => {});
  expect(calls.some(c => c.command === 'agent_chat_new')).toBe(true); expect(sends()).toEqual([]);
  expect(screen.getByText('Hello back')).toBeTruthy();
});
test('a long tool trace stays bounded while every event remains available on demand', async () => {
  const tools = Array.from({ length: 50 }, (_, index) => ({ name: `tool-${index}`, callId: `call-${index}`, phase: 'Succeeded' }));
  snapshot = { ...idle, versions: [{ turns: [{ ...turn, tools }] }] }; await open();
  const activity = screen.getByRole('list', { name: 'Tool activity' });
  expect(within(activity).getAllByRole('listitem')).toHaveLength(1);
  expect(within(activity).getByText('tool-49')).toBeTruthy();
  const more = screen.getByText('49 more tool activities').closest('details')!;
  expect(more.open).toBe(false);
  expect(within(more).getAllByRole('listitem', { hidden: true })).toHaveLength(49);
  expect(within(more).getByText('tool-0')).toBeTruthy();
  expect(screen.getByText('Hello back').closest('details')).toBeNull();
  fireEvent.click(more.querySelector('summary')!); expect(more.open).toBe(true);
});
test('state announcement stays the same when only polled tools change', async () => {
  snapshot = { ...idle, state: 'working', busy: true, canSend: false, versions: [{ turns: [turn] }] };
  await open(); const announcement = screen.getByRole('status'); const before = announcement.textContent;
  snapshot.versions[0].turns[0].tools.push({ name: 'tool', phase: 'Started', callId: '1' }); await poll();
  expect(announcement.textContent).toBe(before);
});
test('an older status reply cannot roll back a newer mutation', async () => {
  let finish!: (value: unknown) => void; let polls = 0;
  mockIPC(command => {
    if (command === 'agent_chat_status' && ++polls === 2) return new Promise(resolve => { finish = resolve; });
    if (command === 'agent_chat_send') return { ...idle, state: 'working', busy: true, canSend: false, versions: [{ turns: [turn] }] };
    return idle;
  });
  await open(); await poll(); enter(); await act(async () => {});
  await act(async () => finish(idle)); expect(screen.getByText('Hello back')).toBeTruthy();
});
test('document switch ignores a late command from the previous document', async () => {
  let finish!: (value: unknown) => void; send = () => new Promise(resolve => { finish = resolve; });
  const view = await open(); enter(); await act(async () => {});
  const other = 'a7f34ba2-b9bd-4b55-b69d-f0a84a1bf528'; snapshot = { ...idle, documentId: other };
  view.rerender(<Chat key={other} documentId={other} />); await act(async () => {});
  await act(async () => finish({ ...idle, versions: [{ turns: [turn] }] }));
  expect(screen.queryByText('Hello back')).toBeNull(); expect(field().value).toBe('');
});
test('polling is single-flight even when the previous read has not replied', async () => {
  let finish!: (value: unknown) => void; let reads = 0;
  mockIPC(() => { reads++; return new Promise(resolve => { finish = resolve; }); });
  render(<Chat documentId={id} />); await poll(); await poll(); expect(reads).toBe(1);
  await act(async () => finish(idle)); await poll(); expect(reads).toBe(2);
});
test('hidden Chat never follows or tracks the shared page scroll', async () => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  const view = render(<main className="page-content"><Chat documentId={id} active={false} /></main>); await act(async () => {});
  const page = screen.getByRole('main');
  Object.defineProperties(page, { scrollHeight: { value: 1000, configurable: true }, clientHeight: { value: 200, configurable: true }, scrollTop: { value: 800, writable: true } });
  await poll(); expect(page.scrollTop).toBe(800);
  view.rerender(<main className="page-content"><Chat documentId={id} active /></main>); await act(async () => {});
  expect(page.scrollTop).toBe(1000);
  view.rerender(<main className="page-content"><Chat documentId={id} active={false} /></main>);
  page.scrollTop = 400; fireEvent.scroll(page); await poll(); expect(page.scrollTop).toBe(400);
  view.rerender(<main className="page-content"><Chat documentId={id} active /></main>); await poll();
  expect(page.scrollTop).toBe(1000);
});
test('transcript focus clears the current composer after native focus scrolling, without moving visible controls', async () => {
  const { page, summary, dock, flush } = await focusScroll();
  summary.focus();
  expect(page.scrollTop).toBe(100);
  // Model the browser's own focus scroll before the animation frame runs.
  page.scrollTop = 150; summary.getBoundingClientRect = () => ({ bottom: 600 }) as DOMRect;
  flush(); expect(page.scrollTop).toBe(230);
  field().focus();
  // A taller draft, narrower window or extra status/error copy raises the dock.
  dock.getBoundingClientRect = () => ({ top: 440 }) as DOMRect;
  summary.focus(); flush(); expect(page.scrollTop).toBe(390);
  field().focus(); summary.getBoundingClientRect = () => ({ bottom: 400 }) as DOMRect;
  summary.focus(); flush(); expect(page.scrollTop).toBe(390);
});
test('transcript focus scrolling cancels stale focus and inactive/unmounted work without changing shared main styles', async () => {
  const { page, summary, view, flush, cancel } = await focusScroll();
  summary.focus(); field().focus(); flush(); expect(page.scrollTop).toBe(100);
  summary.focus();
  view.rerender(<main className="page-content" style={{ scrollPaddingBottom: '13px' }}><Chat documentId={id} active={false} /></main>);
  expect(cancel).toHaveBeenCalled(); flush(); expect(page.scrollTop).toBe(100);
  field().focus(); summary.focus(); flush(); expect(page.scrollTop).toBe(100);
  expect(page.style.scrollPaddingBottom).toBe('13px');
  view.rerender(<main className="page-content" style={{ scrollPaddingBottom: '13px' }}><Chat documentId={id} /></main>);
  page.scrollTop = 100; fireEvent.scroll(page); field().focus(); summary.focus();
  const before = cancel.mock.calls.length; view.unmount();
  expect(cancel.mock.calls.length).toBeGreaterThan(before);
  flush(); expect(page.scrollTop).toBe(100);
});
async function focusScroll() {
  snapshot = { ...idle, versions: [{ turns: [{ ...turn, tools: [
    { name: 'first', callId: '1', phase: 'Succeeded' }, { name: 'last', callId: '2', phase: 'Succeeded' },
  ] }] }] };
  const frames = new Map<number, FrameRequestCallback>(); let next = 0;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++next, callback); return next; });
  const cancel = vi.fn((frame: number) => { frames.delete(frame); }); vi.stubGlobal('cancelAnimationFrame', cancel);
  const view = render(<main className="page-content" style={{ scrollPaddingBottom: '13px' }}><Chat documentId={id} /></main>);
  await act(async () => {});
  const page = screen.getByRole('main'); const summary = screen.getByText('1 more tool activity').closest('summary')!;
  const dock = field().closest('form')!;
  Object.defineProperties(page, { scrollHeight: { value: 1600 }, clientHeight: { value: 700 }, scrollTop: { value: 100, writable: true } });
  page.getBoundingClientRect = () => ({ top: 60, bottom: 760 }) as DOMRect;
  summary.getBoundingClientRect = () => ({ bottom: 700 }) as DOMRect;
  dock.getBoundingClientRect = () => ({ top: 520 }) as DOMRect;
  fireEvent.scroll(page);
  return { page, summary, dock, view, cancel, flush: () => { const callbacks = [...frames.values()]; frames.clear(); callbacks.forEach(callback => callback(0)); } };
}
test.each(['busy', 'contextUnavailable'])('a definite %s Send rejection never enables follow, including while admission is pending', async error => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  let reject!: (error: string) => void; send = () => new Promise((_resolve, fail) => { reject = fail; });
  render(<main className="page-content"><Chat documentId={id} /></main>); await act(async () => {});
  const page = screen.getByRole('main');
  Object.defineProperties(page, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 }, scrollTop: { value: 100, writable: true } });
  fireEvent.scroll(page); enter('Next question'); await poll();
  expect(page.scrollTop).toBe(100);
  await act(async () => reject(error)); await poll();
  expect(field().value).toBe('Next question'); expect(screen.getByRole('alert')).toBeTruthy();
  expect(page.scrollTop).toBe(100);
});
test.each(['preflight', 'turn'])('an uncertain Send leaves earlier reading alone until polling reconciles its %s', async reconciliation => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  send = () => Promise.reject('unknown transport error');
  render(<main className="page-content"><Chat documentId={id} /></main>); await act(async () => {});
  const page = screen.getByRole('main');
  Object.defineProperties(page, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 }, scrollTop: { value: 100, writable: true } });
  fireEvent.scroll(page); enter('Next question'); await act(async () => {}); await poll();
  expect(page.scrollTop).toBe(100);
  snapshot = reconciliation === 'preflight' ? { ...snapshot, state: 'preparing', busy: true, canSend: false }
    : { ...idle, versions: [{ turns: [turn, { ...turn, message: 'Next question' }] }] };
  await poll(); expect(page.scrollTop).toBe(1000);
});
test.each(['preflight', 'turn'])('pending Send ignores an older %s read and suppresses polling until its RPC settles', async evidence => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  let finishRead!: (value: ChatSnapshot) => void; let rejectSend!: (error: string) => void; let reads = 0;
  mockIPC(command => {
    if (command === 'agent_chat_status' && ++reads === 2) return new Promise(resolve => { finishRead = resolve; });
    if (command === 'agent_chat_send') return new Promise((_resolve, reject) => { rejectSend = reject; });
    return structuredClone(snapshot);
  });
  render(<main className="page-content"><Chat documentId={id} /></main>); await act(async () => {});
  const page = screen.getByRole('main');
  Object.defineProperties(page, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 }, scrollTop: { value: 100, writable: true } });
  fireEvent.scroll(page); await poll(); enter('Next question'); await act(async () => {});
  snapshot = evidence === 'preflight' ? { ...snapshot, state: 'preparing', busy: true, canSend: false }
    : { ...idle, versions: [{ turns: [turn, { ...turn, message: 'Next question' }] }] };
  await act(async () => finishRead(snapshot)); await poll();
  expect(reads).toBe(2); expect(page.scrollTop).toBe(100); expect(field().value).toBe('Next question');
  expect(screen.queryByText('Your agent is working…')).toBeNull();
  expect(screen.getAllByText('Hello back')).toHaveLength(1);
  await act(async () => rejectSend('unknown transport error')); await poll();
  expect(reads).toBe(3); expect(page.scrollTop).toBe(1000);
  page.scrollTop = 100; fireEvent.scroll(page); await poll(); expect(page.scrollTop).toBe(100);
});
test.each([
  { initial: 800, deferred: false }, { initial: 100, deferred: false },
  { initial: 800, deferred: true }, { initial: 100, deferred: true },
])('accepted Send preserves reading after submission from $initial (deferred event: $deferred)', async ({ initial, deferred }) => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  let finish!: (value: ChatSnapshot) => void; send = () => new Promise(resolve => { finish = resolve; });
  render(<main className="page-content"><Chat documentId={id} /></main>); await act(async () => {});
  const page = screen.getByRole('main');
  Object.defineProperties(page, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 }, scrollTop: { value: initial, writable: true } });
  fireEvent.scroll(page); enter('Next question');
  page.scrollTop = 50; if (!deferred) fireEvent.scroll(page);
  snapshot = { ...snapshot, state: 'preparing', busy: true, canSend: false };
  await act(async () => finish(snapshot)); expect(page.scrollTop).toBe(50);
  fireEvent.scroll(page);
  await poll(); expect(page.scrollTop).toBe(50);
  snapshot = { ...idle, versions: [{ turns: [turn, { ...turn, message: 'Next question' }] }] };
  await poll(); expect(page.scrollTop).toBe(50); expect(field().value).toBe('');
});
test.each([
  { reconciliation: 'preflight', deferred: false }, { reconciliation: 'turn', deferred: false },
  { reconciliation: 'preflight', deferred: true }, { reconciliation: 'turn', deferred: true },
])('uncertain Send preserves new reading before $reconciliation reconciliation (deferred event: $deferred)', async ({ reconciliation, deferred }) => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  let reject!: (error: string) => void; send = () => new Promise((_resolve, fail) => { reject = fail; });
  render(<main className="page-content"><Chat documentId={id} /></main>); await act(async () => {});
  const page = screen.getByRole('main');
  Object.defineProperties(page, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 }, scrollTop: { value: 800, writable: true } });
  fireEvent.scroll(page); enter('Next question');
  await act(async () => reject('unknown transport error'));
  page.scrollTop = 50; if (!deferred) { fireEvent.scroll(page); await poll(); expect(page.scrollTop).toBe(50); }
  snapshot = reconciliation === 'preflight' ? { ...snapshot, state: 'preparing', busy: true, canSend: false }
    : { ...idle, versions: [{ turns: [turn, { ...turn, message: 'Next question' }] }] };
  await poll(); expect(page.scrollTop).toBe(50);
  fireEvent.scroll(page);
  snapshot = { ...idle, versions: [{ turns: [turn, { ...turn, message: 'Next question' }] }] };
  await poll(); expect(page.scrollTop).toBe(50); expect(field().value).toBe('');
});
test.each([false, true])('queued pre-submission scroll resumes unless the reader moved away and back (moved: %s)', async moved => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  let finish!: (value: ChatSnapshot) => void; send = () => new Promise(resolve => { finish = resolve; });
  render(<main className="page-content"><Chat documentId={id} /></main>); await act(async () => {});
  const page = screen.getByRole('main');
  Object.defineProperties(page, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 }, scrollTop: { value: 100, writable: true } });
  enter('Next question'); fireEvent.scroll(page);
  if (moved) { page.scrollTop = 50; fireEvent.scroll(page); page.scrollTop = 100; fireEvent.scroll(page); }
  snapshot = { ...idle, versions: [{ turns: [turn, { ...turn, message: 'Next question' }] }] };
  await act(async () => finish(snapshot)); expect(page.scrollTop).toBe(moved ? 100 : 1000);
  await poll(); expect(page.scrollTop).toBe(moved ? 100 : 1000);
});
test('returning to the end while Send is pending keeps current follow intent', async () => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  let finish!: (value: ChatSnapshot) => void; send = () => new Promise(resolve => { finish = resolve; });
  render(<main className="page-content"><Chat documentId={id} /></main>); await act(async () => {});
  const page = screen.getByRole('main');
  Object.defineProperties(page, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 }, scrollTop: { value: 800, writable: true } });
  fireEvent.scroll(page); enter('Next question');
  page.scrollTop = 50; fireEvent.scroll(page); page.scrollTop = 800; fireEvent.scroll(page);
  snapshot = { ...idle, versions: [{ turns: [turn, { ...turn, message: 'Next question' }] }] };
  await act(async () => finish(snapshot)); expect(page.scrollTop).toBe(1000);
  await poll(); expect(page.scrollTop).toBe(1000);
});
test('reading earlier messages after admission stays put through preflight polls and the appended turn', async () => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  send = () => (snapshot = { ...snapshot, state: 'preparing', busy: true, canSend: false });
  render(<main className="page-content"><Chat documentId={id} /></main>); await act(async () => {});
  const page = screen.getByRole('main');
  Object.defineProperties(page, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 }, scrollTop: { value: 100, writable: true } });
  fireEvent.scroll(page); enter('Next question'); await act(async () => {}); expect(page.scrollTop).toBe(1000);
  page.scrollTop = 100; fireEvent.scroll(page); await poll(); expect(page.scrollTop).toBe(100);
  snapshot = { ...idle, versions: [{ turns: [turn, { ...turn, message: 'Next question' }] }] };
  await poll(); expect(page.scrollTop).toBe(100);
});
test('sending from earlier messages resumes follow while passive updates leave the reader alone', async () => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  render(<main className="page-content"><Chat documentId={id} /></main>); await act(async () => {});
  const page = document.querySelector<HTMLElement>('.page-content')!;
  Object.defineProperties(page, { scrollHeight: { value: 1000, configurable: true }, clientHeight: { value: 200, configurable: true }, scrollTop: { value: 100, writable: true } });
  fireEvent.scroll(page); enter('Next question'); await act(async () => {});
  expect(sends()).toHaveLength(1);
  expect(page.scrollTop).toBe(1000);
});
test('an accepted long turn follows before a queued programmatic scroll event can disable it', async () => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  render(<Profiler id="chat" onRender={() => {
    if (document.querySelectorAll('.chat-turn').length > 1) fireEvent.scroll(document.querySelector('.page-content')!);
  }}><main className="page-content"><Chat documentId={id} /></main></Profiler>);
  await act(async () => {});
  const page = document.querySelector<HTMLElement>('.page-content')!;
  Object.defineProperties(page, { scrollHeight: { get: () => document.querySelectorAll('.chat-turn').length > 1 ? 1800 : 1000, configurable: true }, clientHeight: { value: 200 }, scrollTop: { value: 100, writable: true } });
  fireEvent.scroll(page);
  send = () => ({ ...idle, versions: [{ turns: [turn, { ...turn, message: 'A long next turn', answer: 'A long answer' }] }] });
  enter('A long next turn'); await act(async () => {});
  expect(page.scrollTop).toBe(1800);
});
test('the empty live region stays rendered before its first status update', async () => {
  await open();
  const style = document.createElement('style');
  style.textContent = readFileSync('src/app.css', 'utf8'); document.head.append(style);
  try { expect(getComputedStyle(screen.getByRole('status', { hidden: true })).display).not.toBe('none'); }
  finally { style.remove(); }
});
test('completion remains announced to screen readers without routine visible Ready copy', async () => {
  snapshot = { ...idle, state: 'working', busy: true, canSend: false, versions: [{ turns: [{ ...turn, answer: null, phase: 'Running' }] }] };
  await open(); expect(screen.getByRole('status').textContent).toContain('working');
  snapshot = { ...idle, versions: [{ turns: [turn] }] }; await poll();
  const status = screen.getByRole('status');
  expect(status.textContent).toBe('Ready for your message.');
  expect(status.classList.contains('sr-only')).toBe(true);
});
test('conversation follows the page scroll only while the reader stays near its end', async () => {
  snapshot = { ...idle, versions: [{ turns: [turn] }] };
  render(<main className="page-content"><Chat documentId={id} /></main>); await act(async () => {});
  const page = screen.getByRole('main');
  Object.defineProperties(page, { scrollHeight: { value: 1000, configurable: true }, clientHeight: { value: 200, configurable: true }, scrollTop: { value: 800, writable: true } });
  fireEvent.scroll(page); snapshot.versions[0].turns.push({ ...turn, message: 'Later' }); await poll();
  expect(page.scrollTop).toBe(1000);
  page.scrollTop = 100; fireEvent.scroll(page);
  snapshot.versions[0].turns.push({ ...turn, message: 'Still later' }); await poll();
  expect(page.scrollTop).toBe(100);
});
const blockedReason = 'Wait for your changes to save.';
const newConversation = () => screen.getByRole('button', { name: 'New conversation' });
test('blocked idle Chat does not claim Ready and disables New conversation with the exact reason', async () => {
  await open(blockedReason);
  expect(screen.getByRole('status').textContent).not.toMatch(/ready/i);
  expect(newConversation().hasAttribute('disabled')).toBe(true); expect(newConversation().getAttribute('title')).toBe(blockedReason);
  fireEvent.click(newConversation()); await act(async () => {});
  expect(calls.some(c => c.command === 'agent_chat_new')).toBe(false);
});
test('blocked unavailable history shows the blocker, not advice to use the disabled New conversation', async () => {
  snapshot = { ...idle, state: 'contextUnavailable', error: 'identity' };
  await open(blockedReason);
  expect(newConversation().hasAttribute('disabled')).toBe(true); expect(newConversation().getAttribute('title')).toBe(blockedReason);
  expect(screen.getByRole('status').textContent).not.toMatch(/new conversation/i);
  expect(screen.getByRole('alert').textContent).toContain(blockedReason); expect(screen.getByRole('alert').textContent).not.toMatch(/Use New conversation/);
  fireEvent.click(newConversation()); await act(async () => {}); expect(calls.some(c => c.command === 'agent_chat_new')).toBe(false);
});
test('unblocked unavailable history still points to New conversation', async () => {
  snapshot = { ...idle, state: 'contextUnavailable', error: 'identity' }; await open();
  expect(screen.getByRole('status').textContent).toContain('Start a new conversation when available.');
  expect(screen.getByRole('alert').textContent).toContain('Use New conversation');
});
test('blocked active Chat keeps authoritative progress and Stop', async () => {
  snapshot = { ...idle, state: 'working', busy: true, canSend: false, versions: [{ turns: [{ ...turn, answer: null, phase: 'Running' }] }] };
  await open(blockedReason);
  expect(screen.getByRole('status').textContent).toBe('Your agent is working…');
  expect(newConversation().getAttribute('title')).toBe(blockedReason);
  fireEvent.click(screen.getByRole('button', { name: 'Stop' })); await act(async () => {});
  expect(calls.some(c => c.command === 'agent_chat_stop')).toBe(true);
});
test('blocked observation loss keeps its warning and Reconnect', async () => {
  snapshot = { ...idle, state: 'observationLost', canSend: false, versions: [{ turns: [{ ...turn, answer: null }] }] };
  await open(blockedReason);
  expect(screen.getByRole('status').textContent).toMatch(/connection was lost/i);
  fireEvent.click(screen.getByRole('button', { name: 'Reconnect' })); await act(async () => {});
  expect(calls.some(c => c.command === 'agent_chat_reconnect')).toBe(true);
});
test('a blocked parent keeps an uncertain Send status instead of replacing it', async () => {
  send = () => Promise.reject('unknown transport error'); const view = await open(); enter(); await act(async () => {});
  view.rerender(<Chat documentId={id} blocked={blockedReason} />); await act(async () => {});
  expect(screen.getByRole('status').textContent).toBe('Checking whether your message was accepted. Do not resend it.');
  expect(screen.getByRole('button', { name: 'Reconnect' })).toBeTruthy(); expect(newConversation().getAttribute('title')).toBe(blockedReason);
});
