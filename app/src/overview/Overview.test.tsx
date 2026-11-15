import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Overview from './Overview';

const id = '9b2b93c4-375c-4815-9717-39852a96e411';
const installation = '12345678-1234-1234-1234-123456789abc';
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const file = { documentId: id, name: 'reader', state: 'readable', source, summary: { name: 'reader', description: 'Reads a page', model: 'gpt-test', tools: ['web_search'] }, reason: null };
const files = { state: 'complete', files: [file], runActive: false };
const agent = { name: 'bridge-run-123', namespace: 'orka-system', uid: 'uid-1' };
const live = { state: 'complete', installation, agents: [agent], observedAt: 1234567890000, observation: null, runActive: false, reason: null };
const observation = { installation, documentId: id, source, namespace: agent.namespace, agentName: agent.name, agentUid: agent.uid, observedAt: 1234567890000, state: 'complete', report: { verdict: 'passed' } };
const unavailable = { state: 'unavailable', installation: null, agents: [], observedAt: null, observation: null, runActive: false, reason: 'stopped' };
const noop = () => {};
const props = { active: true, runBusy: false, workspaceKey: 'ready', onOpenDocument: noop, onOpenExample: noop, onChooseWorkspace: noop };
beforeEach(() => { vi.stubGlobal('isTauri', true); });
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
function ipc(saved: unknown = files, agents: unknown = live) {
  mockIPC(command => command === 'overview_files' ? saved : agents);
}

test('initial loading is not empty and saved files do not wait for live reads', async () => {
  mockIPC(command => command === 'overview_files' ? files : new Promise(() => {}));
  render(<Overview {...props} />);
  expect(screen.getByText('Reading saved files…')).toBeTruthy();
  expect(screen.queryByText('No agents on this computer yet')).toBeNull();
  expect(await screen.findByRole('heading', { name: 'reader' })).toBeTruthy();
  expect(screen.getByText('Reading agents…')).toBeTruthy();
});

test('counts keep files separate from Agents and external metadata is escaped', async () => {
  ipc({ ...files, files: [{ ...file, summary: { ...file.summary, description: '<img src=x onerror=alert(1)>', tools: ['x'.repeat(256)] } }] });
  const counts = vi.fn(); render(<Overview {...props} onCounts={counts} />);
  expect(await screen.findByText('1 saved file · 1 agent')).toBeTruthy();
  expect(screen.getByText('<img src=x onerror=alert(1)>')).toBeTruthy();
  expect(document.querySelector('img')).toBeNull();
  expect(screen.getByText(/Model in file: gpt-test/)).toBeTruthy();
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 saved file · 1 agent'));
});

test('both empty offers the existing example, unavailable is not empty', async () => {
  ipc({ ...files, files: [] }, { ...live, agents: [] });
  const open = vi.fn(); const view = render(<Overview {...props} onOpenExample={open} />);
  await userEvent.click(await screen.findByRole('button', { name: 'Open example' })); expect(open).toHaveBeenCalledOnce();
  ipc({ ...files, files: [] }, unavailable); view.rerender(<Overview {...props} workspaceKey="closed" />);
  expect(await screen.findByText('Local cluster stopped')).toBeTruthy();
  expect(screen.getByText('No saved agent files')).toBeTruthy();
  expect(screen.queryByText('No agents on this computer yet')).toBeNull();
});

test.each(['stopped', 'image', 'read'])('files remain openable when live list is %s', async reason => {
  ipc(files, { ...unavailable, reason });
  const open = vi.fn(); render(<Overview {...props} onOpenDocument={open} />);
  await userEvent.click(await screen.findByRole('button', { name: 'Open reader' })); expect(open).toHaveBeenCalledWith(id);
  expect(screen.getByText(reason === 'image' ? 'Agent list unavailable with this Mac image' : reason === 'stopped' ? 'Local cluster stopped' : 'Could not read agents')).toBeTruthy();
  expect(screen.queryByText('0 agents')).toBeNull();
  expect(screen.getByText('Check result unavailable for this saved version')).toBeTruthy();
});

test('exact native identity attaches checks and suppresses only that current file', async () => {
  ipc(files, { ...live, observation }); render(<Overview {...props} />);
  expect(await screen.findByText('Checks passed in this session')).toBeTruthy();
  expect(screen.getAllByRole('heading', { name: 'reader' })).toHaveLength(1);
  expect(screen.queryByText('Saved file')).toBeNull();
});

test('a cancelled local observation keeps the agent list readable with neutral status', async () => {
  ipc(files, { ...live, observation: { ...observation, report: { verdict: 'cancelled' } } });
  render(<Overview {...props} />);
  expect(await screen.findByText('Checks cancelled in this session')).toBeTruthy();
  expect(screen.getByText('1 saved file · 1 agent')).toBeTruthy();
  expect(screen.queryByText('Could not read agents')).toBeNull();
  const card = screen.getByRole('heading', { name: 'reader' }).closest('.long-card')!;
  expect(card.querySelector('.status-passed, .status-failed')).toBeNull();
  expect(card.querySelector('.status-not-started')).toBeTruthy();
});

test.each([
  ['uid', { ...observation, agentUid: 'different' }],
  ['installation', { ...observation, installation: '87654321-1234-1234-1234-123456789abc' }],
  ['namespace', { ...observation, namespace: 'elsewhere' }],
])('same name cannot associate a different %s', async (_, incorrect) => {
  ipc(files, { ...live, agents: [{ ...agent, name: 'reader' }], observation: incorrect }); render(<Overview {...props} />);
  expect(await screen.findByText('Not tested by Bridge in this session')).toBeTruthy();
  expect(screen.queryByText('Checks passed in this session')).toBeNull();
  expect(screen.getByText('Saved file')).toBeTruthy();
});

test('changed file keeps a stale link and separate saved version; identical bytes keep document IDs', async () => {
  ipc({ ...files, files: [{ ...file, source: { ...source, tests: 'c'.repeat(64) } }, { ...file, documentId: '22345678-1234-1234-1234-123456789abc', name: 'other' }] }, { ...live, observation });
  render(<Overview {...props} />);
  expect(await screen.findByText('Checks not run since your change')).toBeTruthy();
  expect(screen.getAllByText('Saved file')).toHaveLength(2);
  expect(screen.queryByText('Checks passed in this session')).toBeNull();
});

test('missing current source cannot present a retained pass as current', async () => {
  ipc({ ...files, files: [] }, { ...live, observation }); render(<Overview {...props} />);
  expect(await screen.findByText('Current saved version unavailable')).toBeTruthy();
  expect(screen.queryByText('Checks passed in this session')).toBeNull();
});

test('unassociated Open expands read-only identity details without navigation', async () => {
  ipc(); const open = vi.fn(); render(<Overview {...props} onOpenDocument={open} />);
  const title = await screen.findByRole('heading', { name: agent.name });
  const card = title.closest('.long-card')! as HTMLElement;
  await userEvent.click(within(card).getByText('Open'));
  expect(within(card).getByText('UID')).toBeTruthy();
  expect(within(card).getByText('uid-1')).toBeTruthy();
  expect(open).not.toHaveBeenCalled();
});

test('visible Run defers live requests and repeated Refresh coalesces after release, not report arrival', async () => {
  let calls = 0;
  mockIPC(command => { if (command === 'overview_files') return { ...files, runActive: true }; if (command === 'overview_agents') calls++; return live; });
  const view = render(<Overview {...props} runBusy />);
  await screen.findByRole('heading', { name: 'reader' });
  const user = userEvent.setup(); await user.click(screen.getByRole('button', { name: 'Refresh' })); await user.click(screen.getByRole('button', { name: 'Refresh' }));
  expect(calls).toBe(0); expect(screen.getByText('Refresh pending until checks finish')).toBeTruthy();
  mockIPC(command => { if (command === 'overview_files') return files; if (command === 'overview_agents') calls++; return live; });
  view.rerender(<Overview {...props} />);
  expect(await screen.findByRole('heading', { name: agent.name })).toBeTruthy(); expect(calls).toBe(1);
});

test('hidden post-Run completion does not read live, returning does', async () => {
  let calls = 0; mockIPC(command => command === 'overview_files' ? files : (command === 'overview_agents' && calls++, live));
  const view = render(<Overview {...props} active={false} runBusy />);
  view.rerender(<Overview {...props} active={false} />); await act(async () => {}); expect(calls).toBe(0);
  view.rerender(<Overview {...props} />); await screen.findByRole('heading', { name: agent.name }); expect(calls).toBe(1);
});

test('brief non-Run Busy retries once, read failure never retries as Busy', async () => {
  let calls = 0; mockIPC(command => command === 'overview_files' ? files : (++calls === 1 ? { ...unavailable, state: 'busy', reason: null } : live));
  render(<Overview {...props} />); await screen.findByRole('heading', { name: agent.name }); expect(calls).toBe(2);
});

test('retains Last seen on failure but clears it on workspace lifecycle change and ignores late replies', async () => {
  ipc(); const view = render(<Overview {...props} />); await screen.findByRole('heading', { name: agent.name });
  let resolve!: (value: unknown) => void; mockIPC(command => command === 'overview_files' ? files : new Promise(r => { resolve = r; }));
  await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(resolve).toBeTypeOf('function'));
  ipc(files, { ...unavailable, reason: 'read' });
  view.rerender(<Overview {...props} workspaceKey="closed" />);
  await screen.findByText('Could not read agents');
  await act(async () => { resolve(live); });
  expect(screen.queryByRole('heading', { name: agent.name })).toBeNull();
  expect(screen.queryByText(/Last seen/)).toBeNull();
});

test('same workspace failures preserve a successful observation as Last seen', async () => {
  ipc(); render(<Overview {...props} />); await screen.findByRole('heading', { name: agent.name });
  ipc(files, { ...unavailable, reason: 'read' }); await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  expect(await screen.findByText(/Last seen/)).toBeTruthy(); expect(screen.getByRole('heading', { name: agent.name })).toBeTruthy();
});
