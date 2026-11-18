import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Overview from './Overview';

const id = '9b2b93c4-375c-4815-9717-39852a96e411';
const id2 = '22345678-1234-1234-1234-123456789abc';
const id3 = '32345678-1234-1234-1234-123456789abc';
const installation = '12345678-1234-1234-1234-123456789abc';
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const edited = { agent: 'a'.repeat(64), tests: 'c'.repeat(64) };
const runAgent = { installation, namespace: 'orka-system', agentUid: 'run-uid' };
const result = (verdict: string, passed: number, total: number, over: object = {}) => ({ installation, documentId: id, source, namespace: 'orka-system', agentName: 'bridge-run-1', agentUid: 'run-uid', observedAt: 1234567890000, state: 'complete', report: { verdict, passed, total }, ...over });
const file = { documentId: id, name: 'reader', state: 'readable', source, summary: { name: 'reader', description: 'Reads a page', model: 'gpt-test', tools: ['web_search'] }, reason: null, localResult: result('passed', 3, 3) };
const named = (documentId: string, name: string, over: object = {}) => ({ ...file, documentId, name, summary: { ...file.summary, name }, ...over });
const files = (list: unknown[] = [file], over: object = {}) => ({ state: 'complete', files: list, runActive: false, historyState: 'complete', runAgents: [runAgent], ...over });
const live = (agents: unknown[] = [], over: object = {}) => ({ state: 'complete', installation, agents, observedAt: 1234567890000, observation: null, runActive: false, reason: null, ...over });
const unavailable = { state: 'unavailable', installation: null, agents: [], observedAt: null, observation: null, runActive: false, reason: 'stopped' };
const none = { places: [], historyState: 'complete', unobserved: [] };
const noop = () => {};
const props = { active: true, runBusy: false, localAiConnected: true, workspaceKey: 'ready', onOpenDocument: noop, onChatDocument: noop, onRunDocument: noop, onOpenExample: noop, onChooseWorkspace: noop };
beforeEach(() => { vi.stubGlobal('isTauri', true); });
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
function ipc(saved: unknown = files(), agents: unknown = live(), places: unknown = none) {
  mockIPC(command => command === 'overview_files' ? saved : command === 'overview_places' ? places : agents);
}
const card = (name: RegExp | string) => screen.findByRole('article', { name });
const buttons = (el: HTMLElement) => within(el).getAllByRole('button').map(b => b.textContent);

test.each(['Starting the local cluster', 'Getting your workspace ready…', 'Stopping the local cluster', 'Setup failed'])('workspace status wins over a stopped inventory: %s', async workspaceNote => {
  ipc(files(), unavailable);
  render(<Overview {...props} workspaceNote={workspaceNote} />);
  expect(await screen.findByText(workspaceNote)).toBeTruthy();
  expect(screen.queryByText(/The workspace is stopped/)).toBeNull();
  expect(buttons(await card(/^reader/))).toEqual(['Open', 'Details']);
});

test('setup status blocks readiness actions even if the last live inventory is complete', async () => {
  ipc(); const view = render(<Overview {...props} />); await card(/^reader/);
  view.rerender(<Overview {...props} workspaceNote="Starting the local cluster" />);
  expect(buttons(await card(/^reader/))).toEqual(['Open', 'Details']);
});

test('generated Chat runtime agents are hidden without hiding saved, remote or similarly named agents', async () => {
  const generated = `bridge-chat-agent-${'a'.repeat(32)}`;
  const ownFile = named(id2, generated, { localResult: null });
  const runtime = (name: string, uid: string) => ({ name, namespace: 'orka-system', uid });
  const remote = { key: 'team', context: 'team', service: 'control/orka', namespace: 'agents', stale: false, observedAt: 1234567890000,
    agents: [{ ...runtime(generated, 'remote'), namespace: 'agents', agentType: 'runtime', model: null, tools: null, record: null }] };
  ipc(files([file, ownFile]), live([runtime(generated, 'chat'), runtime('bridge-chat-agent-mine', 'mine')]), { ...none, places: [remote] });
  const counts = vi.fn(); render(<Overview {...props} onCounts={counts} />);
  await card(/^bridge-chat-agent-mine$/);
  expect(screen.getAllByRole('heading', { name: generated })).toHaveLength(2);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('4 agents'));
  await userEvent.click(screen.getByRole('button', { name: 'By agent' }));
  expect(screen.getAllByRole('article')).toHaveLength(4);
});

test.each([
  [files([]), 'No saved agents yet'],
  [files(), '1 saved agent'],
  [{ state: 'unavailable', files: [], runActive: false }, 'Saved files unavailable'],
])('an unvisited hidden Overview reports saved inventory only: %s', async (saved, label) => {
  const counts = vi.fn(); const commands: string[] = [];
  mockIPC(command => { commands.push(command); return command === 'overview_files' ? saved : none; });
  render(<Overview {...props} active={false} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith(label));
  expect(commands).toEqual(['overview_files']);
});

test('activation discards an obsolete initial hidden read', async () => {
  let finish!: (value: unknown) => void; const counts = vi.fn();
  mockIPC(command => command === 'overview_files' ? new Promise(resolve => { finish = resolve; }) : none);
  const view = render(<Overview {...props} active={false} onCounts={counts} />);
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  ipc(files([file, named(id2, 'new-agent', { localResult: null })]));
  view.rerender(<Overview {...props} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('2 agents'));
  await act(async () => finish(files([])));
  expect(counts).toHaveBeenLastCalledWith('2 agents');
});

test('registration refreshes visited Overview counts while hidden without reading live agents', async () => {
  const counts = vi.fn(); ipc(); const view = render(<Overview {...props} filesRevision={0} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent'));
  view.rerender(<Overview {...props} active={false} filesRevision={0} onCounts={counts} />);
  let inventories = 0;
  mockIPC(command => command === 'overview_files' ? files([file, named(id2, 'new-agent', { localResult: null })]) : (inventories++, live()));
  view.rerender(<Overview {...props} active={false} filesRevision={1} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('2 agents'));
  expect(inventories).toBe(0);
});

test('a workspace transition rearms a cancelled registration refresh while Overview stays hidden', async () => {
  const counts = vi.fn(); ipc(); const view = render(<Overview {...props} filesRevision={0} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent'));
  view.rerender(<Overview {...props} active={false} filesRevision={0} onCounts={counts} />);
  let finish!: (value: unknown) => void;
  mockIPC(command => command === 'overview_files' ? new Promise(resolve => { finish = resolve; }) : none);
  view.rerender(<Overview {...props} active={false} filesRevision={1} onCounts={counts} />);
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  ipc(files([file, named(id2, 'new-agent', { localResult: null })]));
  view.rerender(<Overview {...props} active={false} filesRevision={1} workspaceKey="starting" onCounts={counts} />);
  await act(async () => finish(files()));
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('2 agents shown'));
});

test.each(['pending', 'unavailable'])('registration refreshes a visited Overview after its first read was %s', async initial => {
  const counts = vi.fn();
  mockIPC(command => command === 'overview_files' ? initial === 'pending' ? new Promise(() => {}) : { state: 'unavailable', files: [], runActive: false } : none);
  const view = render(<Overview {...props} filesRevision={0} onCounts={counts} />);
  await act(async () => {});
  view.rerender(<Overview {...props} active={false} filesRevision={0} onCounts={counts} />);
  ipc(files());
  view.rerender(<Overview {...props} active={false} filesRevision={1} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent shown'));
});

test('hidden test reads preserve the last final verdict and remember a new final for the next run', async () => {
  const counts = vi.fn(); ipc(files([{ ...file, localResult: result('failed', 0, 1) }]));
  const view = render(<Overview {...props} filesRevision={1} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent, 1 failed'));
  const unknown = { ...file, localResult: result('unknown', 0, 0, { state: 'unknown', report: null }) };
  ipc(files([unknown], { runActive: true }));
  view.rerender(<Overview {...props} active={false} filesRevision={1} activeDocumentId={id} runBusy onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent shown, 1 failed'));
  ipc(files([{ ...file, localResult: result('passed', 1, 1) }]));
  view.rerender(<Overview {...props} active={false} filesRevision={1} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent'));
  ipc(files([unknown], { runActive: true }));
  view.rerender(<Overview {...props} filesRevision={1} activeDocumentId={id} runBusy onCounts={counts} />);
  expect(within(await card(/^reader/)).getByText('Passed')).toBeTruthy();
});

test('registration refreshes an unvisited tab without live discovery', async () => {
  const calls: string[] = []; mockIPC(command => { calls.push(command); return files(); });
  const counts = vi.fn();
  const view = render(<Overview {...props} active={false} filesRevision={0} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 saved agent'));
  view.rerender(<Overview {...props} active={false} filesRevision={1} onCounts={counts} />);
  await act(async () => {}); expect(calls).toEqual(['overview_files', 'overview_files']);
});

test('a hidden registration read cannot overwrite files from a later workspace refresh', async () => {
  ipc(); const view = render(<Overview {...props} filesRevision={0} />); await card(/^reader/);
  view.rerender(<Overview {...props} active={false} filesRevision={0} />);
  let finish!: (value: unknown) => void;
  mockIPC(command => command === 'overview_files' ? new Promise(resolve => { finish = resolve; }) : none);
  view.rerender(<Overview {...props} active={false} filesRevision={1} />);
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  ipc(files([named(id3, 'latest', { localResult: null })]));
  view.rerender(<Overview {...props} filesRevision={1} workspaceKey="new" />);
  await card(/^latest/);
  await act(async () => finish(files([named(id2, 'outdated', { localResult: null })])));
  expect(screen.queryByRole('heading', { name: 'outdated' })).toBeNull();
});

test('initial loading is not empty and saved files do not wait for live reads', async () => {
  mockIPC(command => command === 'overview_files' ? files() : command === 'overview_places' ? none : new Promise(() => {}));
  render(<Overview {...props} />);
  expect(screen.getByText('Reading saved files…')).toBeTruthy();
  expect(screen.queryByText('No agents yet')).toBeNull();
  expect(await card(/^reader/)).toBeTruthy();
  expect(screen.getByText('Reading agents…')).toBeTruthy();
});

test('a card is compact: name, one pill, time, description, one meta line, actions; ids wait behind Details', async () => {
  ipc(); render(<Overview {...props} />);
  const c = await card(/^reader/);
  expect(within(c).getByRole('heading', { name: 'reader' })).toBeTruthy();
  expect(within(c).getByText('Passed')).toBeTruthy();
  expect(c.querySelector('time')?.dateTime).toBe('2009-02-13T23:31:30.000Z');
  expect(within(c).queryByText(/^tested/)).toBeNull();
  expect(within(c).getByText('Reads a page')).toBeTruthy();
  expect(within(c).getByText(/3 of 3 passed/).closest('p')!.textContent).toBe('3 of 3 passed · gpt-test · web_search');
  expect(buttons(c)).toEqual(['Open', 'Chat', 'Run tests', 'Details']);
  expect(c.textContent).not.toMatch(/bridge-run|aaaa|run-uid/);
  expect(within(c).getByRole('button', { name: 'Details' }).getAttribute('aria-expanded')).toBe('false');
  await userEvent.click(within(c).getByRole('button', { name: 'Details' }));
  expect(within(c).getByText('Test run Agent')).toBeTruthy();
  expect(within(c).getByText('bridge-run-1')).toBeTruthy();
  expect(within(c).getByRole('button', { name: 'Copy Agent file hash' })).toBeTruthy();
});

test.each([false, true])('local AI readiness disables actions with one shared reason in either grouping (%s)', async byAgent => {
  ipc();
  const user = userEvent.setup();
  const [open, chat, run] = [vi.fn(), vi.fn(), vi.fn()];
  const own = { ...props, onOpenDocument: open, onChatDocument: chat, onRunDocument: run };
  const view = render(<Overview {...own} localAiConnected={false} />);
  await card(/^reader/);
  if (byAgent) await user.click(screen.getByRole('button', { name: 'By agent' }));
  const reason = screen.getByText('Connect the AI in Clusters to chat or run tests on this computer.');
  expect(reason.tabIndex).toBe(0);
  for (const verb of ['Chat', 'Run tests']) {
    const button = screen.getByRole('button', { name: `${verb} reader` });
    expect(button.matches(':disabled')).toBe(true);
    expect(button.getAttribute('aria-describedby')).toBe(reason.id);
    await user.click(button);
  }
  expect(chat).not.toHaveBeenCalled(); expect(run).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Open reader' }));
  expect(open).toHaveBeenCalledWith(id);
  view.rerender(<Overview {...own} localAiConnected />);
  expect(screen.queryByText(reason.textContent!)).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Chat reader' }));
  await user.click(screen.getByRole('button', { name: 'Run tests reader' }));
  expect(chat).toHaveBeenCalledWith(id); expect(run).toHaveBeenCalledWith(id);
  view.rerender(<Overview {...own} localAiConnected={false} />);
  expect(screen.getByRole('button', { name: 'Run tests reader' }).matches(':disabled')).toBe(true);
  expect(screen.getAllByText(reason.textContent!)).toHaveLength(1);
});

test('actions keep Open, Chat, Run order with consistent button treatments and contextual primary emphasis', async () => {
  const other = named(id2, 'failing', { localResult: result('failed', 0, 2, { documentId: id2 }) });
  const fresh = named(id3, 'fresh', { localResult: null });
  ipc(files([file, other, fresh]), live());
  render(<Overview {...props} />);
  const passed = await card(/^reader/), failed = await card(/^failing/), untested = await card(/^fresh/);
  expect(buttons(passed)).toEqual(['Open', 'Chat', 'Run tests', 'Details']);
  expect(within(passed).getByRole('button', { name: 'Run tests reader' }).className).toContain('button-secondary');
  expect(within(passed).getByRole('button', { name: 'Open reader' }).className).toContain('button-secondary');
  expect(buttons(failed)).toEqual(['Open', 'Run tests', 'Details']);
  expect(within(failed).getByRole('button', { name: 'Open failing' }).className).toContain('button-primary');
  expect(within(untested).getByText('Not tested')).toBeTruthy();
  expect(buttons(untested)).toEqual(['Open', 'Chat', 'Run tests', 'Details']);
  expect(within(untested).getByRole('button', { name: 'Run tests fresh' }).className).toContain('button-primary');
  for (const c of [passed, failed, untested]) expect(c.querySelectorAll('.button-primary').length).toBeLessThanOrEqual(1);
});

test('Open, Chat and Run tests report the document; cards need no optional callbacks', async () => {
  ipc(); const [open, chat, run] = [vi.fn(), vi.fn(), vi.fn()];
  const view = render(<Overview {...props} onOpenDocument={open} onChatDocument={chat} onRunDocument={run} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Open reader' })); await user.click(screen.getByRole('button', { name: 'Chat reader' })); await user.click(screen.getByRole('button', { name: 'Run tests reader' }));
  expect([open, chat, run].map(f => f.mock.calls)).toEqual([[[id]], [[id]], [[id]]]);
  view.rerender(<Overview {...props} onChatDocument={undefined} onRunDocument={undefined} />);
  expect(screen.queryByRole('button', { name: 'Chat reader' })).toBeNull(); expect(screen.queryByRole('button', { name: 'Run tests reader' })).toBeNull();
});

test('each saved file has its own local result, and a failure on earlier files stays Failed and counts', async () => {
  const onEarlier = named(id2, 'moved', { source: edited, localResult: result('failed', 0, 1, { documentId: id2 }) });
  const passedEarlier = named(id3, 'old-pass', { source: edited, localResult: result('passed', 1, 1, { documentId: id3 }) });
  const counts = vi.fn(); ipc(files([file, onEarlier, passedEarlier])); render(<Overview {...props} onCounts={counts} />);
  const failed = await card(/^moved/);
  expect(within(failed).getByText('Failed')).toBeTruthy();
  expect(within(failed).getByText(/0 of 1 passed, before your edit/)).toBeTruthy();
  // A pass on earlier files says nothing about the saved file.
  const stale = await card(/^old-pass/);
  expect(within(stale).getByText('Not run since your edit')).toBeTruthy();
  expect(within(await card(/^reader/)).getByText('Passed')).toBeTruthy();
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('3 agents, 1 failed'));
});

test('a pass recorded on earlier files is Not run since your edit, with Run tests as the next step', async () => {
  ipc(files([{ ...file, source: edited }])); render(<Overview {...props} />);
  const c = await card(/^reader/);
  expect(within(c).getByText('Not run since your edit')).toBeTruthy();
  expect(within(c).getByText(/3 of 3 passed, before your edit/)).toBeTruthy();
  expect(c.querySelector('.status-passed')).toBeNull();
  expect(buttons(c)).toEqual(['Open', 'Chat', 'Run tests', 'Details']);
});

test('Bridge test-run Agents are never cards; exact identity only, never the name', async () => {
  const agents = [{ name: 'bridge-run-1', namespace: 'orka-system', uid: 'run-uid' }, { name: 'bridge-run-1', namespace: 'orka-system', uid: 'user-uid' }, { name: 'reader', namespace: 'elsewhere', uid: 'run-uid' }];
  ipc(files(), live(agents)); render(<Overview {...props} />);
  const stray = await card(/^bridge-run-1$/);
  expect(screen.getAllByRole('heading', { name: 'bridge-run-1' })).toHaveLength(1);
  expect(screen.getByRole('article', { name: /^reader$/ })).toBeTruthy();
  expect(within(stray).getByText('Running on this computer, not saved here')).toBeTruthy();
  expect(within(stray).queryByText(/Passed|Failed|Not tested/)).toBeNull();
  expect(buttons(stray)).toEqual(['Details']);
  await userEvent.click(within(stray).getByRole('button', { name: 'Details' }));
  expect(within(stray).getByText('Name on the cluster')).toBeTruthy(); expect(within(stray).getByText('Namespace')).toBeTruthy();
});

test('a cancelled local observation keeps the agent list readable with neutral status', async () => {
  ipc(files([{ ...file, localResult: result('cancelled', 0, 1) }]), live());
  const counts = vi.fn(); render(<Overview {...props} onCounts={counts} />);
  const c = await card(/^reader/);
  expect(within(c).getByText('Not tested')).toBeTruthy();
  expect(c.querySelector('.status-passed, .status-failed')).toBeNull();
  expect(c.querySelector('.status-not-started')).toBeTruthy();
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent'));
});

test.each([
  ['unavailable history', files([{ ...file, localResult: null }], { historyState: 'unavailable', runAgents: [] })],
  ['older payload without history', { state: 'complete', runActive: false, files: [{ ...file, localResult: undefined }] }],
])('%s hides unowned local Agents, never guessing by name', async (_, saved) => {
  ipc(saved, live([{ name: 'bridge-run-9', namespace: 'orka-system', uid: 'unknown' }]));
  const counts = vi.fn(); render(<Overview {...props} onCounts={counts} />);
  const c = await card(/^reader/);
  expect(within(c).getByText('Result unknown')).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'bridge-run-9' })).toBeNull();
  expect(screen.getByText(/Test history is unavailable/)).toBeTruthy();
  expect(screen.queryByText('No agents yet')).toBeNull();
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent shown'));
});

test('a running local result keeps the previous final result, or is Result unknown, and is never Running', async () => {
  const running = result('unknown', 0, 0, { state: 'running', report: null });
  ipc(files([{ ...file, localResult: result('failed', 1, 2) }])); const view = render(<Overview {...props} />);
  expect(within(await card(/^reader/)).getByText('Failed')).toBeTruthy();
  ipc(files([{ ...file, localResult: running }])); await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(screen.getByRole('article', { name: /^reader/ }).textContent).toContain('1 of 2 passed'));
  expect(within(screen.getByRole('article', { name: /^reader/ })).getByText('Failed')).toBeTruthy();
  view.unmount(); ipc(files([{ ...file, localResult: running }])); render(<Overview {...props} />);
  expect(within(await card(/^reader/)).getByText('Result unknown')).toBeTruthy();
  expect(screen.queryByText('Running')).toBeNull();
});

test.each(['passed', 'failed'])('native active Unknown keeps the previous %s result, but terminal Unknown replaces it', async verdict => {
  const label = verdict === 'passed' ? 'Passed' : 'Failed';
  ipc(files([{ ...file, localResult: result(verdict, 1, 2) }])); render(<Overview {...props} activeDocumentId={id} />);
  expect(within(await card(/^reader/)).getByText(label)).toBeTruthy();
  const unknown = { ...file, localResult: result('unknown', 0, 0, { state: 'unknown', report: null, observedAt: 1234567890001 }) };
  ipc(files([unknown], { runActive: true })); await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await screen.findByText(/Updating when the test finishes/);
  expect(within(screen.getByRole('article', { name: /^reader/ })).getByText(label)).toBeTruthy();
  expect(screen.getByText(/1 of 2 passed/)).toBeTruthy();
  ipc(files([unknown])); await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(within(screen.getByRole('article', { name: /^reader/ })).getByText('Result unknown')).toBeTruthy());
  expect(screen.queryByText(/1 of 2 passed/)).toBeNull();
  const other = named(id2, 'other', { localResult: result('unknown', 0, 0, { documentId: id2, state: 'unknown', report: null }) });
  ipc(files([unknown, other], { runActive: true })); await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await screen.findByText(/Updating when the test finishes/);
  expect(within(screen.getByRole('article', { name: /^reader/ })).getByText('Result unknown')).toBeTruthy();
  expect(within(screen.getByRole('article', { name: /^reader/ })).queryByText(/1 of 2 passed/)).toBeNull();
});

test('an unseen terminal Unknown for A is not replaced by a cached pass while B is active', async () => {
  ipc(); const view = render(<Overview {...props} />);
  expect(within(await card(/^reader/)).getByText('Passed')).toBeTruthy();
  view.rerender(<Overview {...props} active={false} />);
  const unknown = { ...file, localResult: result('unknown', 0, 0, { state: 'unknown', report: null, observedAt: 1234567890001 }) };
  const other = named(id2, 'other', { localResult: result('unknown', 0, 0, { documentId: id2, state: 'unknown', report: null }) });
  ipc(files([unknown, other], { runActive: true }));
  view.rerender(<Overview {...props} activeDocumentId={id2} runBusy />);
  await waitFor(() => expect(within(screen.getByRole('article', { name: /^reader/ })).getByText('Result unknown')).toBeTruthy());
  expect(within(screen.getByRole('article', { name: /^reader/ })).queryByText(/of 3 passed/)).toBeNull();
});

test('file metadata from outside is escaped text', async () => {
  ipc(files([{ ...file, summary: { ...file.summary, description: '<img src=x onerror=alert(1)>', tools: ['x'.repeat(256)] } }]));
  render(<Overview {...props} />);
  expect(await screen.findByText('<img src=x onerror=alert(1)>')).toBeTruthy(); expect(document.querySelector('img')).toBeNull();
});

test('a local test in progress keeps the last good Agents and says why, with Open only', async () => {
  const agents = [{ name: 'mine', namespace: 'orka-system', uid: 'mine-uid' }];
  ipc(files(), live(agents)); const view = render(<Overview {...props} />);
  await screen.findByRole('heading', { name: 'mine' });
  mockIPC(command => command === 'overview_files' ? files([file], { runActive: true }) : command === 'overview_places' ? none : live([]));
  view.rerender(<Overview {...props} runBusy />);
  expect(await screen.findByText(/Updating when the test finishes/)).toBeTruthy();
  expect(screen.getByRole('heading', { name: 'mine' })).toBeTruthy();
  expect(buttons(screen.getByRole('article', { name: /^reader/ }))).toEqual(['Open', 'Details']);
  expect(screen.queryByText('No agents yet')).toBeNull();
});

test('stopped workspace keeps file cards with Open only; empty stays empty with the trouble line', async () => {
  ipc(files(), unavailable); const view = render(<Overview {...props} />);
  expect(await screen.findByText('The workspace is stopped. Start it to test or chat.')).toBeTruthy();
  expect(buttons(await card(/^reader/))).toEqual(['Open', 'Details']);
  const choose = vi.fn(); view.rerender(<Overview {...props} onChooseWorkspace={choose} />);
  await userEvent.click(screen.getByRole('button', { name: 'Open Clusters' })); expect(choose).toHaveBeenCalled();
  ipc(files([]), unavailable); await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  expect(await screen.findByRole('heading', { name: 'No agents yet' })).toBeTruthy();
  expect(screen.getByText('The workspace is stopped. Start it to test or chat.')).toBeTruthy();
});

test('image-limited listing keeps saved results and full actions', async () => {
  ipc(files(), { ...unavailable, reason: 'image' }); render(<Overview {...props} />);
  await card(/^reader/);
  expect(screen.queryByText(/can't list its agents/)).toBeNull();
  const c = await card(/^reader/); expect(within(c).getByText('Passed')).toBeTruthy(); expect(buttons(c)).toEqual(['Open', 'Chat', 'Run tests', 'Details']);
});

test('a failed read is unknown, never an empty onboarding card', async () => {
  ipc(files([]), { ...unavailable, reason: 'read' }); render(<Overview {...props} />);
  expect(await screen.findByText("Couldn't check just now.")).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'No agents yet' })).toBeNull();
  ipc({ state: 'unavailable', files: [], runActive: false }, live()); await userEvent.click(screen.getAllByRole('button', { name: 'Refresh' })[0]);
  expect((await screen.findAllByText(/Couldn't read your saved files/)).length).toBeGreaterThan(0);
  expect(screen.queryByRole('heading', { name: 'No agents yet' })).toBeNull();
});

test.each(['loading', 'unavailable', 'unconnected'])('incomplete %s inventory labels the card count as shown, not a confirmed total', async state => {
  const counts = vi.fn();
  mockIPC(command => {
    if (command === 'overview_files') return files([]);
    if (command === 'overview_places') return state === 'unconnected' ? { ...none, places: [{ key: 'team', context: 'team', service: 'control/orka', namespace: 'agents', stale: true, observedAt: null, agents: [] }] } : none;
    return state === 'loading' ? new Promise(() => {}) : state === 'unavailable' ? unavailable : live();
  });
  render(<Overview {...props} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('0 agents shown'));
});

test('truly empty offers New agent and web-reader, hides grouping and has no Earlier Sends', async () => {
  ipc(files([]), live(), { places: [], historyState: 'complete', unobserved: [] });
  const [create, example] = [vi.fn(), vi.fn()]; render(<Overview {...props} onNewAgent={create} onOpenExample={example} />);
  await screen.findByRole('heading', { name: 'No agents yet' });
  expect(screen.getAllByRole('button', { name: 'New agent' })).toHaveLength(1);
  await userEvent.click(screen.getByRole('button', { name: 'New agent' })); await userEvent.click(screen.getByRole('button', { name: 'Open web-reader' }));
  expect(create).toHaveBeenCalledOnce(); expect(example).toHaveBeenCalledOnce();
  expect(screen.queryByRole('group', { name: 'Group agents' })).toBeNull(); expect(screen.queryByText('Earlier Sends')).toBeNull();
});

test('visible Run defers live requests and repeated Refresh coalesces after release', async () => {
  let calls = 0;
  mockIPC(command => { if (command === 'overview_files') return files([file], { runActive: true }); if (command === 'overview_places') return none; if (command === 'overview_agents') calls++; return live(); });
  const view = render(<Overview {...props} runBusy />);
  await card(/^reader/);
  const user = userEvent.setup(); await user.click(screen.getByRole('button', { name: 'Refresh' })); await user.click(screen.getByRole('button', { name: 'Refresh' }));
  expect(calls).toBe(0);
  mockIPC(command => { if (command === 'overview_files') return files(); if (command === 'overview_places') return none; if (command === 'overview_agents') calls++; return live(); });
  view.rerender(<Overview {...props} />);
  await waitFor(() => expect(calls).toBe(1));
});

test('hidden post-Run completion does not read live, returning does', async () => {
  let calls = 0; mockIPC(command => command === 'overview_files' ? files() : command === 'overview_places' ? none : (command === 'overview_agents' && calls++, live()));
  const view = render(<Overview {...props} active={false} runBusy />);
  view.rerender(<Overview {...props} active={false} />); await act(async () => {}); expect(calls).toBe(0);
  view.rerender(<Overview {...props} />); await waitFor(() => expect(calls).toBe(1));
});

test('brief non-Run Busy retries once, read failure never retries as Busy', async () => {
  let calls = 0; mockIPC(command => command === 'overview_files' ? files() : command === 'overview_places' ? none : (++calls === 1 ? { ...unavailable, state: 'busy', reason: null } : live()));
  render(<Overview {...props} />); await waitFor(() => expect(calls).toBe(2));
});

test('known test identities remain hidden in a cached inventory after native cleanup forgets removed Agents', async () => {
  ipc(files(), live([{ name: 'bridge-run-1', namespace: 'orka-system', uid: 'run-uid' }]));
  render(<Overview {...props} />); await waitFor(() => expect(screen.queryByText('Reading agents…')).toBeNull());
  ipc(files([file], { runActive: true, runAgents: [] }));
  await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await screen.findByText(/Updating when the test finishes/);
  expect(screen.queryByRole('heading', { name: 'bridge-run-1' })).toBeNull();
});

test('failed file refresh retains ownership-filtered Agents, never classifies a new inventory with stale ownership', async () => {
  const counts = vi.fn();
  ipc(files(), live([{ name: 'mine', namespace: 'orka-system', uid: 'mine-uid' }, { name: 'bridge-run-1', namespace: 'orka-system', uid: 'run-uid' }]));
  render(<Overview {...props} onCounts={counts} />); await screen.findByRole('heading', { name: 'mine' });
  let inventories = 0;
  mockIPC(command => {
    if (command === 'overview_files') throw Error('file read failed');
    if (command === 'overview_places') return none;
    inventories++; return live([{ name: 'new-test-Agent', namespace: 'orka-system', uid: 'new-uid' }]);
  });
  await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await screen.findByText(/Couldn't read your saved files just now/);
  expect(screen.getByRole('heading', { name: 'mine' })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'new-test-Agent' })).toBeNull();
  expect(screen.queryByRole('heading', { name: 'bridge-run-1' })).toBeNull();
  expect(inventories).toBe(0);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('2 agents shown'));
  expect(buttons(screen.getByRole('article', { name: /^reader/ }))).toEqual(['Open', 'Details']);
});

test('same workspace failure keeps what Bridge saw; a new workspace lifetime clears it and ignores late replies', async () => {
  const agents = [{ name: 'mine', namespace: 'orka-system', uid: 'mine-uid' }];
  ipc(files(), live(agents)); const view = render(<Overview {...props} />); await screen.findByRole('heading', { name: 'mine' });
  ipc(files(), { ...unavailable, reason: 'read' }); await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  expect(await screen.findByText(/Showing what Bridge saw/)).toBeTruthy(); expect(screen.getByRole('heading', { name: 'mine' })).toBeTruthy();
  let resolve!: (value: unknown) => void; mockIPC(command => command === 'overview_files' ? files() : command === 'overview_places' ? none : new Promise(r => { resolve = r; }));
  await userEvent.click(screen.getByRole('button', { name: 'Refresh' })); await waitFor(() => expect(resolve).toBeTypeOf('function'));
  ipc(files(), { ...unavailable, reason: 'read' }); view.rerender(<Overview {...props} workspaceKey="closed" />);
  await screen.findByText("Couldn't check just now.");
  await act(async () => { resolve(live(agents)); });
  expect(screen.queryByRole('heading', { name: 'mine' })).toBeNull(); expect(screen.queryByText(/Showing what Bridge saw/)).toBeNull();
});
