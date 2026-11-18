import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Overview from './Overview';
import { sendAgentCheck, sendAgentFocus } from '../clusters/send-api';

const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const id = '12345678-1234-1234-1234-123456789abc';
const targetUids = { serviceNamespace: 'control', serviceUid: 'svc', deploymentNamespace: 'control', deploymentUid: 'dep', watchedNamespace: 'agents' };
const record = (uid: string) => ({ id, source, targetDisplay: 'team · agents', targetUids, provider: 'model', plannedAgentName: 'same-name', agentUid: uid, verdict: 'passed', time: 1234567890 });
const agent = (uid: string) => ({ name: 'same-name', namespace: 'agents', uid, agentType: 'native', model: 'm', tools: [], record: record(uid) });
const key = '["team","control","svc","control","dep","agents"]';
const place = (agents: unknown[]) => ({ key, context: 'team', service: 'control/orka', namespace: 'agents', stale: false, observedAt: 1234567890000, agents });
const places = (list: unknown[]) => ({ places: [place(list)], historyState: 'complete', unobserved: [] });
const files = { state: 'complete', files: [], runActive: false, historyState: 'complete', runAgents: [] };
const live = { state: 'complete', installation: id, agents: [], observedAt: 123, observation: null, runActive: false, reason: null };
const props = { active: true, runBusy: false, localAiConnected: true, workspaceKey: 'ready', onOpenDocument: vi.fn(), onOpenExample: vi.fn(), onChooseWorkspace: vi.fn() };
const target = (uid: string) => ({ key, name: 'same-name', namespace: 'agents', uid });
beforeEach(() => { vi.stubGlobal('isTauri', true); });
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); vi.clearAllMocks(); });
function ipc(inventory: unknown, check: unknown = 'present') {
  const calls = vi.fn<(command: string, args?: unknown) => unknown>(command => command === 'overview_places' ? inventory : command === 'overview_files' ? files : command === 'send_agent_check' ? check : live);
  mockIPC(calls); return calls;
}
const focused = () => (document.activeElement as HTMLElement | null)?.dataset.cardKey;
test('two same-name agents with different UIDs: the exact UID card takes focus, refreshing only that place', async () => {
  const calls = ipc(places([agent('uid-one'), agent('uid-two')]));
  const view = render(<Overview {...props} focus={{ id: 1, sendId: 's1', target: target('uid-two') }} />);
  await waitFor(() => expect(focused()).toBe(`${key}/uid-two`));
  expect(calls.mock.calls.some(([command, args]) => command === 'overview_places' && (args as { key?: string } | undefined)?.key === key)).toBe(true);
  expect(calls.mock.calls.some(([command]) => command === 'send_agent_check')).toBe(false);
  // Navigation refreshes only its recorded destination, even while a full list is visible.
  expect(calls.mock.calls.filter(([command]) => command === 'overview_places').every(([, args]) => (args as { key?: string } | undefined)?.key === key)).toBe(true);
  // Repeated navigation focuses the new exact target again.
  view.rerender(<Overview {...props} focus={{ id: 2, sendId: 's2', target: target('uid-one') }} />);
  await waitFor(() => expect(focused()).toBe(`${key}/uid-one`));
});
test.each([false, true])('completion during the first scoped read still reveals the exact target without resetting filters (user moved: %s)', async moved => {
  let release!: (value: unknown) => void; let scopedReads = 0;
  const stale = places([agent('uid-one')]);
  const fresh = places([agent('uid-one'), { ...agent('uid-two'), record: { ...record('uid-two'), verdict: 'failed' } }]);
  const calls = vi.fn<(command: string, args?: unknown) => unknown>((command, args) => command === 'overview_places' ? (args as { key?: string } | undefined)?.key ? ++scopedReads === 1 ? new Promise(resolve => { release = resolve; }) : fresh : stale : command === 'overview_files' ? files : live);
  mockIPC(calls);
  const view = render(<Overview {...props} onNewAgent={() => undefined} />);
  await screen.findByRole('article', { name: 'same-name' });
  const focus = { id: 1, sendId: id, target: target('uid-two') };
  view.rerender(<Overview {...props} onNewAgent={() => undefined} focus={focus} placesRevision={0} />);
  await waitFor(() => expect(release).toBeDefined());
  const filter = screen.getByRole('textbox', { name: 'Filter by name' }) as HTMLInputElement;
  fireEvent.change(filter, { target: { value: 'same' } });
  fireEvent.click(screen.getByRole('button', { name: 'team', pressed: false }));
  const control = screen.getByRole('button', { name: 'New agent' });
  if (moved) control.focus();
  view.rerender(<Overview {...props} onNewAgent={() => undefined} focus={focus} placesRevision={1} />);
  await screen.findByRole('img', { name: 'team: Failed' });
  await waitFor(() => moved ? expect(document.activeElement).toBe(control) : expect(focused()).toBe(`${key}/uid-two`));
  const row = screen.getByRole('img', { name: 'team: Failed' }).closest('article')!;
  expect(within(row).getByRole('button', { name: 'Show places for same-name' }).getAttribute('aria-expanded')).toBe('true');
  expect(screen.getByRole('article', { name: 'team, Failed' }).dataset.cardKey).toBe(`${key}/uid-two`);
  expect(filter.value).toBe('same');
  expect(screen.getByRole('button', { name: 'team', pressed: true })).toBeTruthy();
  await act(async () => release(stale));
  expect(screen.getByRole('article', { name: 'team, Failed' }).dataset.cardKey).toBe(`${key}/uid-two`);
  if (moved) expect(document.activeElement).toBe(control);
  else expect(focused()).toBe(`${key}/uid-two`);
  expect(calls.mock.calls.filter(([command]) => command === 'overview_places').map(([, args]) => args)).toEqual([{}, { key }, { key }]);
  expect(calls.mock.calls.some(([command]) => command === 'send_agent_check')).toBe(false);
});
test.each(['revealed', 'expanded', 'collapsed'])('completion revalidates the requested place without resetting %s disclosure, filters or user focus', async disclosure => {
  const other = { ...place([{ ...agent('uid-other'), name: 'other-agent' }]), key: 'other', context: 'other' };
  let inventory = { ...places([agent('uid-one')]), places: [place([agent('uid-one')]), other] };
  const calls = vi.fn<(command: string, args?: unknown) => unknown>(command => command === 'overview_places' ? inventory : command === 'overview_files' ? files : live);
  mockIPC(calls);
  // S2 is admitted but not journalled yet: navigation to S1 sees only the old terminal result, so no poll starts.
  const focus = { id: 1, sendId: id, target: target('uid-one') };
  const view = render(<Overview {...props} onNewAgent={() => undefined} focus={focus} placesRevision={0} />);
  await waitFor(() => expect(focused()).toBe(`${key}/uid-one`));
  expect(screen.getByRole('img', { name: 'team: Passed' })).toBeTruthy();
  const filter = screen.getByRole('textbox', { name: 'Filter by name' }) as HTMLInputElement;
  if (disclosure !== 'revealed') {
    fireEvent.click(screen.getByRole('button', { name: 'team', pressed: false }));
    fireEvent.change(filter, { target: { value: 'same' } });
    if (disclosure === 'expanded') fireEvent.click(screen.getByRole('button', { name: 'Show places for same-name' }));
  }
  const before = screen.queryByRole('article', { name: 'team, Passed' });
  if (before) fireEvent.click(within(before).getByRole('button', { name: 'Details' }));
  const control = screen.getByRole('button', { name: 'New agent' }); control.focus();
  inventory = { ...inventory, places: [place([{ ...agent('uid-one'), record: { ...record('uid-one'), id: '22345678-1234-1234-1234-123456789abc', source: { ...source, agent: 'c'.repeat(64) }, verdict: 'failed' } }]), other] };
  view.rerender(<Overview {...props} onNewAgent={() => undefined} focus={focus} placesRevision={1} />);
  await screen.findByRole('img', { name: 'team: Failed' });
  expect(filter.value).toBe(disclosure === 'revealed' ? '' : 'same');
  expect(screen.getByRole('button', { name: disclosure === 'revealed' ? 'All' : 'team', pressed: true })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Show places for same-name' }).getAttribute('aria-expanded')).toBe(String(disclosure !== 'collapsed'));
  if (before) {
    expect(screen.getByRole('article', { name: 'team, Failed' })).toBe(before);
    expect(within(before).getByRole('button', { name: 'Details' }).getAttribute('aria-expanded')).toBe('true');
    expect(within(before).getByText('c'.repeat(64))).toBeTruthy();
  } else expect(screen.queryByRole('article', { name: 'team, Failed' })).toBeNull();
  expect(document.activeElement).toBe(control);
  if (disclosure === 'revealed') expect(screen.getByRole('img', { name: 'other: Passed' })).toBeTruthy();
  expect(calls.mock.calls.filter(([command]) => command === 'overview_places').map(([, args]) => args)).toEqual([{ key }, { key }]);
  expect(calls.mock.calls.filter(([command]) => command === 'overview_files')).toHaveLength(1);
  expect(calls.mock.calls.filter(([command]) => command === 'overview_agents')).toHaveLength(1);
  expect(calls.mock.calls.some(([command]) => command === 'send_agent_check')).toBe(false);
});
test('a late scoped reply cannot undo a newer completion revision for the same focus identity', async () => {
  let release!: (value: unknown) => void; let reads = 0;
  const passed = places([agent('uid-one')]);
  const failed = places([{ ...agent('uid-one'), record: { ...record('uid-one'), verdict: 'failed' } }]);
  const calls = vi.fn<(command: string, args?: unknown) => unknown>(command => command === 'overview_places' ? ++reads === 2 ? new Promise(resolve => { release = resolve; }) : reads === 1 ? passed : failed : command === 'overview_files' ? files : live);
  mockIPC(calls);
  const focus = { id: 1, sendId: id, target: target('uid-one') };
  const view = render(<Overview {...props} focus={focus} placesRevision={0} />);
  await waitFor(() => expect(focused()).toBe(`${key}/uid-one`));
  view.rerender(<Overview {...props} focus={focus} placesRevision={1} />);
  await waitFor(() => expect(release).toBeDefined());
  view.rerender(<Overview {...props} focus={focus} placesRevision={2} />);
  await screen.findByRole('img', { name: 'team: Failed' });
  await act(async () => release(passed));
  expect(screen.getByRole('img', { name: 'team: Failed' })).toBeTruthy();
  expect(screen.queryByRole('img', { name: 'team: Passed' })).toBeNull();
  expect(calls.mock.calls.filter(([command]) => command === 'overview_places').map(([, args]) => args)).toEqual([{ key }, { key }, { key }]);
});
test('a late local inventory preserves the exact place node and destination focus', async () => {
  let release!: (value: unknown) => void;
  const inventory = { ...places([agent('uid-one')]), places: [{ ...place([agent('a'), agent('b'), agent('c')]), key: 'other', context: 'other' }, place([agent('uid-one')])] };
  mockIPC(command => command === 'overview_places' ? inventory : command === 'overview_files' ? new Promise(resolve => { release = resolve; }) : live);
  render(<Overview {...props} focus={{ id: 1, sendId: 's1', target: target('uid-one') }} />);
  await screen.findAllByRole('article', { name: /^same-name$/ });
  const before = [...document.querySelectorAll<HTMLElement>('[data-card-key]')].find(node => node.dataset.cardKey === `${key}/uid-one`);
  expect(before).toBeTruthy();
  await act(async () => release({ ...files, files: Array.from({ length: 6 }, (_, index) => ({ documentId: `00000000-0000-4000-8000-00000000001${index}`, name: `saved-${index}`, state: 'readable', source, summary: null, reason: null, localResult: null })) }));
  await screen.findByRole('article', { name: /^saved-0/ });
  expect(before?.isConnected).toBe(true);
  await waitFor(() => expect(focused()).toBe(`${key}/uid-one`));
});
test.each([false, true])('post-run inventory updates preserve only an owned focus (user moved: %s)', async moved => {
  let release!: (value: unknown) => void;
  const inventory = { ...places([agent('uid-one')]), places: [{ ...place([agent('a'), agent('b'), agent('c')]), key: 'other', context: 'other' }, place([agent('uid-one')])] };
  mockIPC(command => command === 'overview_places' ? inventory : command === 'overview_files' ? files : new Promise(resolve => { release = resolve; }));
  const focus = { id: 1, sendId: 's1', target: target('uid-one') };
  const view = render(<Overview {...props} runBusy onNewAgent={() => undefined} focus={focus} />);
  await waitFor(() => expect(focused()).toBe(`${key}/uid-one`));
  const before = document.activeElement as HTMLElement;
  const newAgent = screen.getByRole('button', { name: 'New agent' });
  if (moved) newAgent.focus();
  view.rerender(<Overview {...props} onNewAgent={() => undefined} focus={focus} />);
  await waitFor(() => expect(release).toBeDefined());
  await act(async () => release({ ...live, agents: Array.from({ length: 6 }, (_, index) => ({ name: `local-${index}`, namespace: 'agents', uid: `local-${index}` })) }));
  await screen.findByRole('article', { name: /^local-0/ });
  expect(before.isConnected).toBe(true);
  await waitFor(() => moved ? expect(document.activeElement).toBe(newAgent) : expect(focused()).toBe(`${key}/uid-one`));
});
test.each(['lookup', 'destination', 'local'] as const)('a later focus choice wins while the %s read is pending', async slow => {
  let release!: (value: unknown) => void;
  mockIPC(command => command === 'overview_places' ? slow === 'destination' ? new Promise(resolve => { release = resolve; }) : places([agent('uid-one')]) : command === 'overview_files' ? slow === 'local' ? new Promise(resolve => { release = resolve; }) : files : live);
  const focus = { id: 1, sendId: 's1', target: target('uid-one') };
  const view = render(<Overview {...props} onNewAgent={() => undefined} focus={slow === 'lookup' ? { id: 1, sendId: 's1' } : focus} focusPending={slow === 'lookup'} />);
  if (slow !== 'lookup') await waitFor(() => expect(release).toBeDefined());
  const control = await screen.findByRole('button', { name: 'New agent' });
  control.focus();
  if (slow === 'lookup') view.rerender(<Overview {...props} onNewAgent={() => undefined} focus={focus} />);
  else await act(async () => release(slow === 'local' ? files : places([agent('uid-one')])));
  await screen.findByRole('article', { name: /^same-name$/ });
  expect(document.activeElement).toBe(control);
});
test.each([false, true])('compact resize preserves only an owned exact-agent focus (user moved: %s)', async moved => {
  vi.stubGlobal('innerWidth', 1280);
  ipc(places([agent('uid-one')]));
  render(<Overview {...props} onNewAgent={() => undefined} focus={{ id: 1, sendId: 's1', target: target('uid-one') }} />);
  await waitFor(() => expect(focused()).toBe(`${key}/uid-one`));
  const before = document.activeElement as HTMLElement;
  const control = screen.getByRole('button', { name: 'New agent' });
  if (moved) control.focus();
  vi.stubGlobal('innerWidth', 540); fireEvent.resize(window);
  expect(before.isConnected).toBe(true);
  await waitFor(() => moved ? expect(document.activeElement).toBe(control) : expect(focused()).toBe(`${key}/uid-one`));
});
test('an older focus reply cannot supersede a newer navigation', async () => {
  let release!: (value: unknown) => void;
  let slow = true;
  vi.stubGlobal('isTauri', true);
  mockIPC((command, args) => command === 'overview_places' ? (slow && (args as { key?: string } | undefined)?.key ? (slow = false, new Promise(resolve => { release = resolve; })) : places([agent('uid-one'), agent('uid-two')])) : command === 'overview_files' ? files : live);
  const view = render(<Overview {...props} focus={{ id: 1, sendId: 's1', target: target('uid-one') }} />);
  await waitFor(() => expect(release).toBeDefined());
  view.rerender(<Overview {...props} focus={{ id: 2, sendId: 's2', target: target('uid-two') }} />);
  await waitFor(() => expect(focused()).toBe(`${key}/uid-two`));
  release(places([agent('uid-one'), agent('uid-two')]));
  await new Promise(resolve => setTimeout(resolve, 50));
  expect(focused()).toBe(`${key}/uid-two`);
});
test.each([
  ['present', 'Not shown in Overview yet.'],
  ['gone', 'The original agent is no longer available. Your saved result is unchanged.'],
  ['cannotCheck', 'Cannot check this agent. Your saved result is unchanged.'],
])('a missing card is checked read-only and reported as %s, never assumed deleted', async (check, text) => {
  const calls = ipc(places([agent('uid-other')]), check);
  render(<Overview {...props} focus={{ id: 1, sendId: 's1', target: target('uid-one') }} />);
  expect(await screen.findByText(text)).toBeTruthy();
  expect(within(screen.getByText(text).closest('.overview-note') as HTMLElement).getByRole('button', { name: 'Refresh' })).toBeTruthy();
  expect(calls.mock.calls.find(([command]) => command === 'send_agent_check')?.[1]).toEqual({ sendId: 's1' });
});
test.each([
  ['present', 'Not shown in Overview yet.'],
  ['gone', 'The original agent is no longer available. Your saved result is unchanged.'],
  ['cannotCheck', 'Cannot check this agent. Your saved result is unchanged.'],
])('completion during an absence check restores %s guidance without resetting user choices', async (check, text) => {
  let release!: (value: unknown) => void; let checks = 0;
  let inventory = places([agent('uid-other')]);
  mockIPC(command => command === 'overview_places' ? inventory : command === 'overview_files' ? files : command === 'send_agent_check' ? ++checks === 1 ? new Promise(resolve => { release = resolve; }) : check : live);
  const focus = { id: 1, sendId: id, target: target('uid-one') };
  const view = render(<Overview {...props} onNewAgent={() => undefined} focus={focus} placesRevision={0} />);
  await waitFor(() => expect(release).toBeDefined());
  const filter = screen.getByRole('textbox', { name: 'Filter by name' }) as HTMLInputElement;
  fireEvent.change(filter, { target: { value: 'same' } });
  fireEvent.click(screen.getByRole('button', { name: 'team', pressed: false }));
  const row = screen.getByRole('article', { name: 'same-name' });
  fireEvent.click(within(row).getByRole('button', { name: 'Show places for same-name' }));
  const card = screen.getByRole('article', { name: 'team, Passed' });
  fireEvent.click(within(card).getByRole('button', { name: 'Details' }));
  const control = screen.getByRole('button', { name: 'New agent' }); control.focus();
  const expectChoices = () => {
    expect(filter.value).toBe('same');
    expect(screen.getByRole('button', { name: 'team', pressed: true })).toBeTruthy();
    expect(within(card).getByRole('button', { name: 'Details' }).getAttribute('aria-expanded')).toBe('true');
    expect(within(row).getByRole('button', { name: 'Show places for same-name' }).getAttribute('aria-expanded')).toBe('true');
    expect(card.isConnected).toBe(true);
    expect(document.activeElement).toBe(control);
  };
  view.rerender(<Overview {...props} onNewAgent={() => undefined} focus={focus} placesRevision={1} />);
  const note = await screen.findByText(text);
  expect(within(note.closest('.overview-note') as HTMLElement).getByRole('button', { name: 'Refresh' })).toBeTruthy();
  expectChoices();
  // The target's eventual presence clears the explanation without navigating again.
  inventory = places([agent('uid-other'), agent('uid-one')]);
  view.rerender(<Overview {...props} onNewAgent={() => undefined} focus={focus} placesRevision={2} />);
  await waitFor(() => expect(screen.getAllByRole('article', { name: 'same-name' })).toHaveLength(2));
  const targetRow = screen.getAllByRole('article', { name: 'same-name' }).find(node => node !== row)!;
  expect(within(targetRow).getByRole('button', { name: 'Show places for same-name' }).getAttribute('aria-expanded')).toBe('false');
  expect(screen.queryByText(text)).toBeNull();
  expectChoices();
  await act(async () => release('gone'));
  expect(screen.queryByText(/The original agent is no longer available/)).toBeNull();
  expect(screen.queryByText(text)).toBeNull();
  expectChoices();
});
test('Refresh retries a present-but-unlisted target after completion cancels its first absence check', async () => {
  let release!: (value: unknown) => void; let checks = 0;
  let inventory = places([]);
  mockIPC(command => command === 'overview_places' ? inventory : command === 'overview_files' ? files : command === 'send_agent_check' ? ++checks === 1 ? new Promise(resolve => { release = resolve; }) : 'present' : live);
  const focus = { id: 1, sendId: 's1', target: target('uid-one') };
  const view = render(<Overview {...props} focus={focus} placesRevision={0} />);
  await waitFor(() => expect(release).toBeDefined());
  view.rerender(<Overview {...props} focus={focus} placesRevision={1} />);
  const note = await screen.findByText('Not shown in Overview yet.');
  inventory = places([agent('uid-one')]);
  fireEvent.click(within(note.closest('.overview-note') as HTMLElement).getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(focused()).toBe(`${key}/uid-one`));
  expect(screen.queryByText('Not shown in Overview yet.')).toBeNull();
  await act(async () => release('gone'));
  expect(screen.queryByText(/The original agent is no longer available/)).toBeNull();
  expect(focused()).toBe(`${key}/uid-one`);
});
test('leaving an agent-specific request clears its warning before ordinary Overview navigation', async () => {
  const calls = ipc(places([]), 'gone');
  const view = render(<Overview {...props} focus={{ id: 1, sendId: 's1', target: target('uid-one') }} />);
  await screen.findByText(/The original agent is no longer available/);
  view.rerender(<Overview {...props} active={false} />);
  view.rerender(<Overview {...props} />);
  await waitFor(() => expect(screen.queryByText(/The original agent is no longer available/)).toBeNull());
  view.rerender(<Overview {...props} active={false} />);
  view.rerender(<Overview {...props} />);
  expect(screen.queryByText(/The original agent is no longer available/)).toBeNull();
  expect(calls.mock.calls.filter(([command]) => command === 'send_agent_check')).toHaveLength(1);
});
test('an explicit inventory reread after navigation reads all clusters instead of refocusing the original target', async () => {
  const calls = ipc(places([agent('uid-one')]));
  const view = render(<Overview {...props} filesRevision={0} focus={{ id: 1, sendId: 's1', target: target('uid-one') }} />);
  await waitFor(() => expect(focused()).toBe(`${key}/uid-one`));
  view.rerender(<Overview {...props} filesRevision={1} />);
  await waitFor(() => expect(calls.mock.calls.some(([command, args]) => command === 'overview_places' && !(args as { key?: string } | undefined)?.key)).toBe(true));
  expect(focused()).toBe(`${key}/uid-one`);
});
test('successful navigation clears an earlier inventory read error even when the target is absent', async () => {
  ipc(undefined);
  const view = render(<Overview {...props} />);
  await screen.findByText(/Couldn't read clusters just now/);
  ipc(places([]), 'present');
  view.rerender(<Overview {...props} focus={{ id: 1, sendId: 's1', target: target('uid-one') }} />);
  await screen.findByText('Not shown in Overview yet.');
  expect(screen.queryByText(/Couldn't read clusters just now/)).toBeNull();
});
test('a failed focused read makes only its place unknown, keeping other fresh results', async () => {
  const other = { ...place([{ ...agent('uid-other'), name: 'other-agent' }]), key: 'other', context: 'other' };
  const calls = ipc({ ...places([agent('uid-one')]), places: [place([agent('uid-one')]), other] }, 'cannotCheck');
  const view = render(<Overview {...props} />);
  const status = async (place: 'team' | 'other') => (await screen.findByRole('article', { name: new RegExp(`^${place}, (Passed|Result unknown)$`) })).textContent;
  fireEvent.click(within(await screen.findByRole('article', { name: 'same-name' })).getByRole('button', { name: 'Show places for same-name' }));
  fireEvent.click(within(await screen.findByRole('article', { name: 'other-agent' })).getByRole('button', { name: 'Show places for other-agent' }));
  expect(await status('team')).toContain('Passed');
  calls.mockImplementation(command => command === 'overview_files' ? files : command === 'send_agent_check' ? 'cannotCheck' : live); calls.mockClear();
  view.rerender(<Overview {...props} focus={{ id: 1, sendId: 's1', target: target('uid-one') }} />);
  await screen.findByText('Cannot check this agent. Your saved result is unchanged.');
  expect(await status('team')).toContain('Result unknown');
  expect(await status('other')).toContain('Passed');
  expect(screen.getByText(/Couldn't read clusters just now/)).toBeTruthy();
  expect(calls.mock.calls.filter(([command]) => command === 'overview_places').every(([, args]) => (args as { key?: string } | undefined)?.key === key)).toBe(true);
});
test('a Send with no recorded agent says so without any check', async () => {
  const calls = ipc(places([agent('uid-one')]));
  render(<Overview {...props} focus={{ id: 1, sendId: 's1' }} />);
  expect(await screen.findByText(/Cannot check this agent/)).toBeTruthy();
  expect(calls.mock.calls.some(([command]) => command === 'send_agent_check')).toBe(false);
});
test('the native agent commands take only a send id and treat anything malformed as not found or cannot check', async () => {
  const calls = vi.fn<(command: string, args?: unknown) => unknown>(command => command === 'send_agent_focus' ? { ...target('uid-one'), extra: 'field' } : 'deleted');
  mockIPC(calls);
  expect(await sendAgentFocus('s1')).toBeUndefined();
  expect(await sendAgentCheck('s1')).toBe('cannotCheck');
  expect(calls.mock.calls.map(([, args]) => args)).toEqual([{ sendId: 's1' }, { sendId: 's1' }]);
  mockIPC(() => target('uid-one'));
  expect(await sendAgentFocus('s1')).toEqual(target('uid-one'));
});
