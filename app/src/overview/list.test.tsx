import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Overview from './Overview';

const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const id = '9b2b93c4-375c-4815-9717-39852a96e411';
const noop = () => {};
const props = { active: true, runBusy: false, localAiConnected: true, workspaceKey: 'ready', onOpenDocument: noop, onChatDocument: noop, onRunDocument: noop, onOpenExample: noop, onChooseWorkspace: noop, onNewAgent: noop };
function file(documentId = id, name = 'reader', verdict = 'passed', time = 1000) {
  return { documentId, name, state: 'readable', source, summary: { name, description: 'Reads a page', model: 'model', tools: [] }, reason: null, localResult: { installation: id, documentId, source, namespace: 'agents', agentName: name, agentUid: 'run', observedAt: time, state: 'complete', report: { verdict, passed: verdict === 'failed' ? 1 : 2, total: 2 } } };
}
function place(key = 'team', verdict = 'failed', documentId: string | null = id) {
  return { key, context: key, service: 'control/orka', namespace: 'agents', stale: false, observedAt: 9000,
    agents: [{ name: 'remote-reader', namespace: 'agents', uid: `${key}-uid`, agentType: 'runtime', model: null, tools: null,
      record: { id, documentId, source, targetDisplay: key, targetUids: { serviceNamespace: 'control', serviceUid: 'svc', deploymentNamespace: 'control', deploymentUid: 'dep', watchedNamespace: 'agents' }, provider: 'team-model', plannedAgentName: 'remote-reader', agentUid: `${key}-uid`, verdict, time: 2, detailState: documentId ? 'saved' : 'removed', report: verdict === 'running' ? null : { verdict, passed: 1, total: 2 } } }] };
}
function ipc(saved = [file()], places = [place()]) {
  mockIPC(command => command === 'overview_files' ? { state: 'complete', files: saved, historyState: 'complete', runAgents: [], runActive: false }
    : command === 'overview_places' ? { places, historyState: 'complete', unobserved: [] }
      : { state: 'complete', installation: id, observedAt: 1000, agents: [], observation: null, runActive: false, reason: null });
}
const row = () => screen.findByRole('article', { name: 'reader' });
beforeEach(() => vi.stubGlobal('isTauri', true));
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });

test('one row per document, one mark per place, one primary; evidence and actions wait behind expansion', async () => {
  ipc(); const counts = vi.fn(); const open = vi.fn(); const run = vi.fn();
  render(<Overview {...props} onCounts={counts} onOpenDocument={open} onRunDocument={run} />);
  const agent = await row();
  expect(screen.queryByRole('button', { name: 'By agent' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'By cluster' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Refresh' })).toBeNull();
  expect(within(agent).getByLabelText('This computer: Passed')).toBeTruthy();
  expect(within(agent).getByLabelText('team: Failed')).toBeTruthy();
  expect(within(agent).getByRole('button', { name: 'Show places for reader', description: /This computer: Passed; team: Failed/ })).toBeTruthy();
  expect(within(agent).queryByText('1 of 2 passed')).toBeNull();
  expect(document.querySelectorAll('.button-primary')).toHaveLength(1);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent, 1 failed'));
  await userEvent.click(within(agent).getByRole('button', { name: 'Open reader' }));
  expect(open).toHaveBeenLastCalledWith(id);
  await userEvent.click(within(agent).getByLabelText('team: Failed'));
  expect(within(agent).getByRole('button', { name: 'Show places for reader' }).getAttribute('aria-expanded')).toBe('true');
  const remote = within(agent).getByRole('article', { name: 'team, Failed' });
  expect(within(remote).getByText('1 of 2 passed')).toBeTruthy();
  await userEvent.click(within(remote).getByRole('button', { name: 'Run tests reader on team' }));
  expect(run).toHaveBeenLastCalledWith(id, 'team');
  expect(within(remote).queryByRole('button', { name: /Open/ })).toBeNull();
});

test('place and name filters intersect, filtering never turns into onboarding', async () => {
  ipc([file(), file('22345678-1234-1234-1234-123456789abc', 'writer')]); render(<Overview {...props} />); await row();
  const results = screen.getByText(/^2 agents/);
  expect(results.getAttribute('role')).toBe('status');
  await userEvent.click(within(screen.getByRole('group', { name: 'Place' })).getByRole('button', { name: 'team' }));
  expect(results.textContent).toMatch(/^1 agent/);
  expect(screen.queryByRole('article', { name: 'writer' })).toBeNull();
  const agent = await row(); expect(within(agent).queryByLabelText('This computer: Passed')).toBeNull();
  await userEvent.type(screen.getByRole('textbox', { name: 'Filter by name' }), 'WRIT');
  expect(screen.queryByRole('article', { name: 'reader' })).toBeNull();
  expect(screen.getByText('No agents match these filters.')).toBeTruthy();
  expect(results.textContent).toMatch(/^0 agents/);
  expect(screen.getAllByRole('status')).toContain(results);
  expect(screen.queryByRole('heading', { name: 'No agents yet' })).toBeNull();
});

test.each([3, 4])('%i clusters plus this computer use a select only past four actual places', async n => {
  ipc([file()], Array.from({ length: n }, (_, i) => place(`team-${i}`))); render(<Overview {...props} />); await row();
  if (n === 4) {
    const select = screen.getByRole('combobox', { name: 'Place' });
    await userEvent.selectOptions(select, 'team-2');
    expect(within(await row()).getByLabelText('team-2: Failed')).toBeTruthy();
    expect(within(await row()).queryByLabelText('This computer: Passed')).toBeNull();
  } else {
    expect(screen.queryByRole('combobox', { name: 'Place' })).toBeNull();
    expect(within(screen.getByRole('group', { name: 'Place' })).getAllByRole('button')).toHaveLength(5);
  }
});

test('unavailable test history can be retried in place without restoring top-level Refresh', async () => {
  let recovered = false;
  const files = vi.fn(() => ({ state: 'complete', historyState: recovered ? 'complete' : 'unavailable', runAgents: [], runActive: false, files: [recovered ? file() : { ...file(), localResult: null }] }));
  mockIPC(command => command === 'overview_files' ? files() : command === 'overview_places' ? { places: [], historyState: 'complete', unobserved: [] }
    : { state: 'complete', installation: id, observedAt: 1000, agents: [], observation: null, runActive: false, reason: null });
  render(<Overview {...props} />);
  await screen.findByText(/Test history is unavailable/);
  recovered = true;
  await userEvent.click(screen.getByRole('button', { name: 'Check again' }));
  await waitFor(() => expect(files).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(screen.queryByText(/Test history is unavailable/)).toBeNull());
  expect(within(await row()).getByLabelText('This computer: Passed')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Refresh' })).toBeNull();
});

test('unavailable Send history has an in-page retry even when place inventory is healthy', async () => {
  let recovered = false;
  const places = vi.fn(() => ({ places: [place()], historyState: recovered ? 'complete' : 'unavailable', unobserved: [] }));
  mockIPC(command => command === 'overview_files' ? { state: 'complete', historyState: 'complete', runAgents: [], runActive: false, files: [file()] }
    : command === 'overview_places' ? places() : { state: 'complete', installation: id, observedAt: 1000, agents: [], observation: null, runActive: false, reason: null });
  render(<Overview {...props} />);
  const notice = await screen.findByRole('alert');
  recovered = true;
  await userEvent.click(within(notice).getByRole('button', { name: 'Check again' }));
  await waitFor(() => expect(places).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(screen.queryByRole('alert')).toBeNull());
  expect(within(await row()).getByLabelText('team: Failed')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Refresh' })).toBeNull();
});

test('a long context keeps its full accessible name while filtering by the opaque place key', async () => {
  const context = 'context-' + 'unbrokensegment'.repeat(16);
  ipc([file()], [{ ...place('opaque-key'), context }]); render(<Overview {...props} />); await row();
  const button = screen.getByRole('button', { name: context });
  await userEvent.click(button);
  expect(button.getAttribute('aria-pressed')).toBe('true');
  expect(within(await row()).getByLabelText(`${context}: Failed`)).toBeTruthy();
  expect(screen.queryByLabelText('This computer: Passed')).toBeNull();
});

test('removing the selected place resets its filter, including when that key returns later', async () => {
  const saved = [file(), file('22345678-1234-1234-1234-123456789abc', 'writer')];
  const clusters = Array.from({ length: 5 }, (_, i) => place(`team-${i}`));
  ipc(saved, clusters); const view = render(<Overview {...props} />); await row();
  await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Place' }), 'team-2');
  expect(screen.queryByRole('article', { name: 'writer' })).toBeNull();
  ipc(saved, clusters.filter(p => p.key !== 'team-2'));
  view.rerender(<Overview {...props} active={false} />); view.rerender(<Overview {...props} />);
  await waitFor(() => expect(screen.getByRole('combobox', { name: 'Place' })).toHaveProperty('value', 'all'));
  expect(await screen.findByRole('article', { name: 'writer' })).toBeTruthy();
  ipc(saved, clusters); view.rerender(<Overview {...props} active={false} />); view.rerender(<Overview {...props} />);
  await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(7));
  expect(screen.getByRole('combobox', { name: 'Place' })).toHaveProperty('value', 'all');
});

test('rows sort by worst visible status, then newest run; equal names never merge', async () => {
  ipc([file(id, 'passed-new', 'passed', 9000), file('22345678-1234-1234-1234-123456789abc', 'failed-old', 'failed', 1), file('32345678-1234-1234-1234-123456789abc', 'stale', 'unknown', 5000), file('42345678-1234-1234-1234-123456789abc', 'passed-new', 'passed', 100)], [place('team', 'running', id)]);
  render(<Overview {...props} />); await screen.findByRole('article', { name: 'failed-old' });
  expect([...document.querySelectorAll('.overview-agent > .overview-agent-head h2')].map(e => e.getAttribute('aria-label'))).toEqual(['failed-old', 'passed-new', 'stale', 'passed-new']);
});

test('AI gating applies only to fitting local action, remote Run and Open stay enabled', async () => {
  ipc(); render(<Overview {...props} localAiConnected={false} />); const agent = await row();
  await userEvent.click(within(agent).getByRole('button', { name: 'Show places for reader' }));
  const local = within(agent).getByRole('article', { name: 'This computer, Passed' });
  const chat = within(local).getByRole('button', { name: 'Chat reader' });
  expect(chat).toHaveProperty('disabled', true);
  expect(document.getElementById(chat.getAttribute('aria-describedby')!)?.textContent).toContain('Connect the AI');
  expect(within(agent).getByRole('button', { name: 'Open reader' })).toHaveProperty('disabled', false);
  expect(within(agent).getByRole('button', { name: 'Run tests reader on team' })).toHaveProperty('disabled', false);
});

test('navigation clears filters and reveals the exact UID among repeated agents in one place', async () => {
  const team = place();
  team.agents.push({ ...team.agents[0], uid: 'second', record: { ...team.agents[0].record, agentUid: 'second' } });
  ipc([file()], [team]);
  const view = render(<Overview {...props} />); const agent = await row();
  expect(within(agent).getAllByLabelText('team: Failed')).toHaveLength(1);
  await userEvent.type(screen.getByRole('textbox', { name: 'Filter by name' }), 'hidden');
  await userEvent.click(within(screen.getByRole('group', { name: 'Place' })).getByRole('button', { name: 'This computer' }));
  view.rerender(<Overview {...props} focus={{ id: 1, sendId: id, target: { key: 'team', uid: 'second', namespace: 'agents', name: 'remote-reader' } }} />);
  await waitFor(() => expect(document.activeElement?.getAttribute('data-card-key')).toBe('team/second'));
  expect(screen.getByRole('textbox', { name: 'Filter by name' })).toHaveProperty('value', '');
  expect(within(await row()).getAllByRole('article', { name: 'team, Failed' })).toHaveLength(2);
  await userEvent.click(within(await row()).getByRole('button', { name: 'Show places for reader' }));
  expect(within(await row()).queryByRole('article', { name: 'team, Failed' })).toBeNull();
});

test.each([false, true])('an active Send becomes a retained failure without leaving Overview (targeted: %s)', async targeted => {
  vi.useFakeTimers();
  let finished = false;
  const inventory = () => {
    const team = place('team', finished ? 'failed' : 'running');
    const other = place('team', 'passed').agents[0];
    team.agents.push({ ...other, uid: 'other-uid', record: { ...other.record, agentUid: 'other-uid' } });
    return { places: [team], historyState: 'complete', unobserved: [] };
  };
  const reads = vi.fn<(args?: unknown) => ReturnType<typeof inventory>>(inventory);
  mockIPC((command, args) => command === 'overview_places' ? reads(args)
    : command === 'overview_files' ? { state: 'complete', files: [file()], historyState: 'complete', runAgents: [], runActive: false }
      : { state: 'complete', installation: id, observedAt: 1000, agents: [], observation: null, runActive: false, reason: null });
  const run = vi.fn();
  const focus = targeted ? { id: 1, sendId: id, target: { key: 'team', uid: 'team-uid', namespace: 'agents', name: 'remote-reader' } } : undefined;
  render(<Overview {...props} focus={focus} onRunDocument={run} />);
  await act(async () => {});
  const agent = screen.getByRole('article', { name: 'reader' });
  expect(within(agent).getByLabelText('team: Running')).toBeTruthy();
  if (!targeted) fireEvent.click(within(agent).getByRole('button', { name: 'Show places for reader' }));
  const remote = within(agent).getByRole('article', { name: 'team, Running' });
  expect(remote.getAttribute('data-card-key')).toBe('team/team-uid');
  if (targeted) expect(document.activeElement).toBe(remote);
  fireEvent.click(within(remote).getByRole('button', { name: 'Details' }));
  finished = true;
  await act(async () => { await vi.advanceTimersByTimeAsync(500); });
  expect(within(agent).getByLabelText('team: Failed')).toBeTruthy();
  expect(within(agent).queryByLabelText('team: Running')).toBeNull();
  expect(within(agent).getByRole('article', { name: 'team, Failed' })).toBe(remote);
  expect(within(remote).getByText('1 of 2 passed')).toBeTruthy();
  expect(within(remote).getByText('team-uid')).toBeTruthy();
  expect(within(remote).getByText(source.agent)).toBeTruthy();
  expect(within(remote).getByText(source.tests)).toBeTruthy();
  expect(reads.mock.calls.every(([args]) => targeted ? (args as { key?: string })?.key === 'team' : (args as { key?: string })?.key === undefined)).toBe(true);
  fireEvent.click(within(remote).getByRole('button', { name: 'Run tests reader on team' }));
  expect(run).toHaveBeenLastCalledWith(id, 'team');
  const stopped = reads.mock.calls.length;
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(reads).toHaveBeenCalledTimes(stopped);
});

test('a later scoped poll clears the resolved absence note without navigating to the found target', async () => {
  vi.useFakeTimers();
  let found = false;
  const team = place('team', 'running');
  const unobserved = { id, source, targetDisplay: 'team', targetUids: team.agents[0].record.targetUids, provider: 'team-model', plannedAgentName: 'remote-reader', agentUid: 'team-uid', verdict: 'running', time: 2 };
  mockIPC(command => command === 'overview_places' ? { places: [found ? team : { ...team, agents: [] }], historyState: 'complete', unobserved: found ? [] : [unobserved] }
    : command === 'send_agent_check' ? 'present'
      : command === 'overview_files' ? { state: 'complete', files: [file()], historyState: 'complete', runAgents: [], runActive: false }
        : { state: 'complete', installation: id, observedAt: 1000, agents: [], observation: null, runActive: false, reason: null });
  const focus = { id: 1, sendId: id, target: { key: 'team', uid: 'team-uid', namespace: 'agents', name: 'remote-reader' } };
  render(<Overview {...props} focus={focus} />);
  await act(async () => {});
  expect(screen.getByText('Not shown in Overview yet.')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Refresh' })).toBeTruthy();
  const filter = screen.getByRole('textbox', { name: 'Filter by name' });
  fireEvent.change(filter, { target: { value: 'reader' } });
  act(() => filter.focus());
  found = true;
  await act(async () => { await vi.advanceTimersByTimeAsync(500); });
  expect(within(screen.getByRole('article', { name: 'reader' })).getByLabelText('team: Running')).toBeTruthy();
  expect(screen.queryByText('Not shown in Overview yet.')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Refresh' })).toBeNull();
  expect(filter).toHaveProperty('value', 'reader');
  expect(document.activeElement).toBe(filter);
  expect(screen.getByRole('button', { name: 'Show places for reader' }).getAttribute('aria-expanded')).toBe('false');
  expect(screen.queryByRole('article', { name: 'team, Running' })).toBeNull();
});

test('an initial absence check arriving after a scoped poll finds the target cannot restore its note', async () => {
  vi.useFakeTimers();
  let found = false;
  let release!: (value: unknown) => void;
  const team = place('team', 'running');
  const unobserved = { id, source, targetDisplay: 'team', targetUids: team.agents[0].record.targetUids, provider: 'team-model', plannedAgentName: 'remote-reader', agentUid: 'team-uid', verdict: 'running', time: 2 };
  mockIPC(command => command === 'overview_places' ? { places: [found ? team : { ...team, agents: [] }], historyState: 'complete', unobserved: found ? [] : [unobserved] }
    : command === 'send_agent_check' ? new Promise(resolve => { release = resolve; })
      : command === 'overview_files' ? { state: 'complete', files: [file()], historyState: 'complete', runAgents: [], runActive: false }
        : { state: 'complete', installation: id, observedAt: 1000, agents: [], observation: null, runActive: false, reason: null });
  const focus = { id: 1, sendId: id, target: { key: 'team', uid: 'team-uid', namespace: 'agents', name: 'remote-reader' } };
  render(<Overview {...props} focus={focus} />);
  await act(async () => {});
  expect(release).toBeDefined();
  expect(screen.queryByLabelText('team: Running')).toBeNull();
  found = true;
  await act(async () => { await vi.advanceTimersByTimeAsync(500); });
  expect(within(screen.getByRole('article', { name: 'reader' })).getByLabelText('team: Running')).toBeTruthy();
  await act(async () => release('gone'));
  expect(screen.queryByText('The original agent is no longer available. Your saved result is unchanged.')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Refresh' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Show places for reader' }).getAttribute('aria-expanded')).toBe('false');
});

test.each([
  ['other place', true, false], ['unrelated unobserved', true, false],
  ['same place', true, true], ['focused unobserved', true, true],
  ['other place', false, true], ['unrelated unobserved', false, true],
] as const)('polling respects %s work (targeted: %s)', async (work, targeted, continues) => {
  vi.useFakeTimers();
  let targetFinished = false; let otherFinished = false; let reads = 0;
  const inventory = () => {
    const team = place('team', targetFinished ? 'failed' : 'running');
    const other = place('other', otherFinished ? 'passed' : 'running');
    other.agents[0].record.id = '22345678-1234-1234-1234-123456789abc';
    if (work === 'same place') team.agents.push({ ...other.agents[0], record: { ...other.agents[0].record, targetDisplay: 'team' } });
    // The focused Send may still be unobserved while this UID retains an earlier result.
    if (work === 'focused unobserved') team.agents[0].record.id = other.agents[0].record.id;
    const unobserved = work.includes('unobserved') ? [{
      id: work === 'focused unobserved' ? id : '22345678-1234-1234-1234-123456789abc', source,
      targetDisplay: 'other', targetUids: other.agents[0].record.targetUids, provider: 'team-model',
      plannedAgentName: 'remote-reader', agentUid: null, verdict: otherFinished ? 'passed' : 'running', time: 2,
    }] : [];
    return { places: work === 'other place' ? [team, other] : [team], historyState: 'complete', unobserved };
  };
  mockIPC(command => command === 'overview_places' ? (reads++, inventory())
    : command === 'overview_files' ? { state: 'complete', files: [file()], historyState: 'complete', runAgents: [], runActive: false }
      : { state: 'complete', installation: id, observedAt: 1000, agents: [], observation: null, runActive: false, reason: null });
  const focus = targeted ? { id: 1, sendId: id, target: { key: 'team', uid: 'team-uid', namespace: 'agents', name: 'remote-reader' } } : undefined;
  render(<Overview {...props} focus={focus} />);
  await act(async () => {});
  if (!targeted) fireEvent.click(screen.getByRole('button', { name: 'Show places for reader' }));
  const remote = screen.getAllByRole('article', { name: 'team, Running' }).find(node => node.getAttribute('data-card-key') === 'team/team-uid');
  expect(remote).toBeTruthy();
  targetFinished = true;
  await act(async () => { await vi.advanceTimersByTimeAsync(500); });
  expect(screen.getByRole('article', { name: 'team, Failed' })).toBe(remote);
  expect(reads).toBe(2);
  await act(async () => { await vi.advanceTimersByTimeAsync(500); });
  expect(reads).toBe(continues ? 3 : 2);
  otherFinished = true;
  await act(async () => { await vi.advanceTimersByTimeAsync(500); });
  const stopped = reads;
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(reads).toBe(stopped);
});

test.each([false, true])('a late poll cannot replace a newer scoped navigation (previously targeted: %s)', async targeted => {
  vi.useFakeTimers();
  let release!: (value: unknown) => void;
  let reads = 0;
  mockIPC(command => command === 'overview_places'
    ? ++reads === 2 ? new Promise(resolve => { release = resolve; }) : { places: [place('team', reads === 1 ? 'running' : 'failed')], historyState: 'complete', unobserved: [] }
    : command === 'overview_files' ? { state: 'complete', files: [file()], historyState: 'complete', runAgents: [], runActive: false }
      : { state: 'complete', installation: id, observedAt: 1000, agents: [], observation: null, runActive: false, reason: null });
  const target = { key: 'team', uid: 'team-uid', namespace: 'agents', name: 'remote-reader' };
  const view = render(<Overview {...props} focus={targeted ? { id: 1, sendId: id, target } : undefined} />);
  await act(async () => {});
  await act(async () => { await vi.advanceTimersByTimeAsync(500); });
  expect(release).toBeDefined();
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(reads).toBe(2); // Slow reads must not overlap.
  view.rerender(<Overview {...props} focus={{ id: 2, sendId: id, target }} />);
  await act(async () => {});
  const remote = screen.getByRole('article', { name: 'team, Failed' });
  expect(document.activeElement).toBe(remote);
  await act(async () => release({ places: [place('team', 'running')], historyState: 'complete', unobserved: [] }));
  expect(screen.getByRole('article', { name: 'team, Failed' })).toBe(remote);
  expect(document.activeElement).toBe(remote);
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(reads).toBe(3);
});

test.each([false, true])('inactive Overview cancels pending polling and ignores its late reply (targeted: %s)', async targeted => {
  vi.useFakeTimers();
  let release!: (value: unknown) => void;
  let reads = 0;
  mockIPC(command => command === 'overview_places'
    ? ++reads === 1 ? { places: [place('team', 'running')], historyState: 'complete', unobserved: [] } : new Promise(resolve => { release = resolve; })
    : command === 'overview_files' ? { state: 'complete', files: [file()], historyState: 'complete', runAgents: [], runActive: false }
      : { state: 'complete', installation: id, observedAt: 1000, agents: [], observation: null, runActive: false, reason: null });
  const focus = targeted ? { id: 1, sendId: id, target: { key: 'team', uid: 'team-uid', namespace: 'agents', name: 'remote-reader' } } : undefined;
  const view = render(<Overview {...props} active={false} focus={focus} />);
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(reads).toBe(0);
  view.rerender(<Overview {...props} focus={focus} />);
  await act(async () => {});
  await act(async () => { await vi.advanceTimersByTimeAsync(500); });
  expect(release).toBeDefined();
  view.rerender(<Overview {...props} active={false} focus={focus} />);
  await act(async () => release({ places: [place('team', 'failed')], historyState: 'complete', unobserved: [] }));
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(reads).toBe(2);
  expect(view.container.querySelector('[aria-label="team: Running"]')).toBeTruthy();
  expect(view.container.querySelector('[aria-label="team: Failed"]')).toBeNull();
});

test.each(['passed', 'failed', 'unknown', 'cancelled'])('a terminal %s Send never starts polling', async verdict => {
  vi.useFakeTimers();
  const team = place('team', verdict);
  if (verdict === 'unknown' || verdict === 'cancelled') team.agents[0].record.report = null;
  let reads = 0;
  mockIPC(command => command === 'overview_places' ? (reads++, { places: [team], historyState: 'complete', unobserved: [] })
    : command === 'overview_files' ? { state: 'complete', files: [file()], historyState: 'complete', runAgents: [], runActive: false }
      : { state: 'complete', installation: id, observedAt: 1000, agents: [], observation: null, runActive: false, reason: null });
  render(<Overview {...props} />);
  await act(async () => {});
  expect(screen.getByRole('article', { name: 'reader' })).toBeTruthy();
  await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
  expect(reads).toBe(1);
});

test('an empty remembered place still explains its empty inventory instead of leaving a blank list', async () => {
  const team = place(); team.agents = [];
  ipc([], [team]); render(<Overview {...props} />);
  expect(await screen.findByText(/No agents in agents yet/)).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'No agents yet' })).toBeNull();
});

test('unlinked inventory never invents last-run time and keeps Details instead of Open', async () => {
  const remote = place('team', 'passed', null); remote.agents[0].record = null as never;
  ipc([], [remote]); render(<Overview {...props} />);
  const agent = await screen.findByRole('article', { name: 'remote-reader' });
  expect(agent.querySelector('time')).toBeNull();
  expect(within(agent).queryByRole('button', { name: /Open/ })).toBeNull();
  const details = within(agent).getByRole('button', { name: 'Details remote-reader' });
  expect(details.getAttribute('aria-expanded')).toBe('false');
  await userEvent.click(details);
  expect(details.getAttribute('aria-expanded')).toBe('true');
  expect(document.getElementById(details.getAttribute('aria-controls')!)).toBeTruthy();
  expect(within(agent).getByRole('article', { name: 'team, No recorded result' })).toBeTruthy();
});
