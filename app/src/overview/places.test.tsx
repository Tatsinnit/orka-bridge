import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Overview from './Overview';

const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const id = '12345678-1234-1234-1234-123456789abc';
const file = { documentId: id, name: 'reader', state: 'readable', source, summary: { name: 'reader', description: 'Reads a page', model: 'declared-model', tools: ['web_fetch'] }, reason: null };
const record = { id, source, targetDisplay: 'team-one · agents', targetUids: { serviceNamespace: 'control', serviceUid: 'svc-one', deploymentNamespace: 'control', deploymentUid: 'dep-one', watchedNamespace: 'agents' }, provider: 'team-model', plannedAgentName: 'same-name', agentUid: 'uid-one', verdict: 'passed', time: 1234567890 };
const agent = { name: 'same-name', namespace: 'agents', uid: 'uid-one', agentType: 'native', model: 'observed-model', tools: ['web_fetch'], record };
const first = { key: 'one', context: 'team-one', service: 'control/orka', namespace: 'agents', stale: false, observedAt: 1234567890000, agents: [agent, { ...agent, uid: 'uid-two', record: { ...record, id: '22345678-1234-1234-1234-123456789abc', agentUid: 'uid-two', verdict: 'unknown' } }, { ...agent, uid: 'external', agentType: 'runtime', record: null }] };
const second = { ...first, key: 'two', context: 'team-two', agents: [{ ...agent, uid: 'uid-three', record: { ...record, agentUid: 'uid-three', targetDisplay: 'team-two · agents', targetUids: { ...record.targetUids, serviceUid: 'svc-two', deploymentUid: 'dep-two' } } }] };
const places = { places: [first, second], historyState: 'complete', unobserved: [] };
const local = { state: 'complete', installation: id, agents: [], observedAt: 123, observation: null, runActive: false, reason: null };
const props = { active: true, runBusy: false, workspaceKey: 'ready', onOpenDocument: vi.fn(), onOpenExample: vi.fn(), onChooseWorkspace: vi.fn() };
function ipc(value: unknown = places, files: unknown[] = [file], live: unknown = local) {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => command === 'overview_places' ? value : command === 'overview_files' ? { state: 'complete', files, runActive: false } : live);
}
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); vi.clearAllMocks(); });

test('two installations in the same context and namespace keep distinct accessible place labels and cross-links', async () => {
  const a = { ...second, key: 'a', context: 'same-team', service: 'control/orka-a' };
  const b = { ...second, key: 'b', context: 'same-team', service: 'control/orka-b' };
  ipc({ ...places, places: [a, b] }); render(<Overview {...props} />);
  expect(await screen.findByRole('region', { name: 'same-team · agents · control/orka-a' })).toBeTruthy();
  expect(screen.getByRole('region', { name: 'same-team · agents · control/orka-b' })).toBeTruthy();
  expect(screen.getByText('Also found on same-team · agents · control/orka-b')).toBeTruthy();
  await userEvent.click(screen.getByRole('button', { name: 'By agent' }));
  const group = screen.getByRole('region', { name: 'reader · saved version' });
  expect(within(group).getAllByRole('heading', { level: 3 })).toHaveLength(2);
  expect(within(group).getAllByText('same-team · agents · control/orka-a').length).toBeGreaterThan(0);
});

test('each retained Agent has a card per place, external runtime is read-only and not blessed by its name', async () => {
  ipc(); render(<Overview {...props} />);
  const one = await screen.findByRole('region', { name: 'team-one · agents · control/orka' });
  expect(within(one).getAllByRole('heading', { name: 'reader' })).toHaveLength(2);
  expect(within(one).getByRole('heading', { name: 'same-name' })).toBeTruthy();
  expect(within(one).getByText('Not tested by Bridge')).toBeTruthy();
  expect(within(one).getByText('Runtime: runtime')).toBeTruthy();
  expect(within(one).getAllByText('Model on cluster: observed-model')).toHaveLength(3);
  const external = within(one).getByRole('heading', { name: 'same-name' }).closest('.long-card')! as HTMLElement;
  expect(within(external).queryByRole('button', { name: /Open/ })).toBeNull();
  await userEvent.click(within(external).getByText('Open'));
  expect(within(external).getByText('external')).toBeTruthy();
  expect(props.onOpenDocument).not.toHaveBeenCalled();
  expect(screen.getAllByText('Also found on team-two · agents · control/orka')).toHaveLength(2);
});

test('By agent joins only known pair hashes across places and local Agent, leaving unassociated identities separate', async () => {
  const observation = { installation: id, documentId: id, source, namespace: 'orka-system', agentName: 'local-reader', agentUid: 'local-uid', observedAt: 123, state: 'complete', report: { verdict: 'passed' } };
  ipc(places, [file], { ...local, agents: [{ name: 'local-reader', namespace: 'orka-system', uid: 'local-uid' }], observation });
  render(<Overview {...props} />);
  await userEvent.click(await screen.findByRole('button', { name: 'By agent' }));
  const revision = screen.getByRole('region', { name: 'reader · saved version' });
  expect(within(revision).getAllByRole('heading', { level: 3 })).toHaveLength(4);
  expect(within(revision).getByText('This computer')).toBeTruthy();
  expect(within(revision).getByText('Also found on team-one · agents · control/orka; team-two · agents · control/orka')).toBeTruthy();
  expect(within(revision).getAllByText('team-two · agents · control/orka').length).toBeGreaterThan(0);
  const unassociated = screen.getByRole('region', { name: 'Agent without a linked file · team-one · agents · control/orka' });
  expect(within(unassociated).getAllByRole('heading', { level: 3 })).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'By agent' }).getAttribute('aria-pressed')).toBe('true');
});

test('different check-file hashes form different revision groups even with identical names', async () => {
  const different = { ...second, agents: [{ ...agent, record: { ...record, source: { ...source, tests: 'c'.repeat(64) } } }] };
  ipc({ ...places, places: [first, different] }); render(<Overview {...props} />);
  await userEvent.click(await screen.findByRole('button', { name: 'By agent' }));
  expect(screen.getByRole('region', { name: 'reader · saved version' })).toBeTruthy();
  expect(screen.getByRole('region', { name: 'Known file · saved version' })).toBeTruthy();
});

test('remote Open uses one exact owned hash binding, but identical files are ambiguous and remain read-only', async () => {
  ipc({ ...places, places: [second] }); const view = render(<Overview {...props} />);
  const remote = await screen.findByRole('region', { name: 'team-two · agents · control/orka' });
  await userEvent.click(within(remote).getByRole('button', { name: 'Open reader' }));
  expect(props.onOpenDocument).toHaveBeenCalledWith(id);
  ipc({ ...places, places: [second] }, [file, { ...file, documentId: '22345678-1234-1234-1234-123456789abc' }]);
  view.rerender(<Overview {...props} workspaceKey="new" />);
  await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  expect(within(remote).queryByRole('button', { name: 'Open reader' })).toBeNull();
  expect(within(remote).getByText('Open')).toBeTruthy();
});

test('offline last-seen cards never present a current green pass and recover only on fresh native result', async () => {
  ipc({ ...places, places: [{ ...second, stale: true }] }); render(<Overview {...props} />);
  const remote = await screen.findByRole('region', { name: 'team-two · agents · control/orka' });
  expect(within(remote).getByText('Earlier cluster check. Refresh to check again.')).toBeTruthy();
  expect(within(remote).getByText(/Last seen/)).toBeTruthy();
  expect(within(remote).getByText('Recorded checks passed; observation stale')).toBeTruthy();
  expect(remote.querySelector('.status-passed')).toBeNull();
  ipc({ ...places, places: [second] }); await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  expect(within(remote).getByText('Checks passed here')).toBeTruthy();
});

test('a place with no successful observation never contributes an invented zero Agent total', async () => {
  ipc({ ...places, places: [{ ...second, checkedAt: 1234567890000, observedAt: null, stale: true, agents: [] }] });
  const counts = vi.fn(); render(<Overview {...props} onCounts={counts} />);
  expect(await screen.findByText('Agents not checked yet. Refresh or check access in Clusters.')).toBeTruthy();
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith(expect.stringContaining('Cluster agent count unavailable')));
  expect(screen.queryByText(/0 cluster Agents/)).toBeNull();
});

test('unknown without UID is an attempt, not a counted Agent or association; history failure is visible', async () => {
  ipc({ places: [], historyState: 'complete', unobserved: [{ ...record, agentUid: null, verdict: 'unknown' }] });
  render(<Overview {...props} />);
  const attempts = await screen.findByRole('region', { name: 'Earlier Sends' });
  expect(within(attempts).getByText('Saved Send attempts. Current cluster status is unknown.')).toBeTruthy();
  expect(within(attempts).queryByText(/not found in the latest check/)).toBeNull();
  expect(within(attempts).getByText('Agent creation not confirmed')).toBeTruthy();
  expect(within(attempts).getByText('Checks unknown')).toBeTruthy();
  expect(within(attempts).queryByRole('button', { name: /Open/ })).toBeNull();
  ipc({ places: [second], historyState: 'unavailable', unobserved: [] });
  await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', 'Saved Send summaries unavailable. Recorded checks cannot be confirmed.');
});

test('cluster inventory refresh works while the local VM is busy and ignores replies after hiding', async () => {
  let resolve!: (value: unknown) => void; ipc();
  mockIPC(command => command === 'overview_places' ? new Promise(r => { resolve = r; }) : { state: 'complete', files: [file], runActive: true });
  const view = render(<Overview {...props} runBusy />);
  await screen.findByRole('heading', { name: 'reader' });
  expect(resolve).toBeTypeOf('function');
  view.rerender(<Overview {...props} active={false} runBusy />);
  await act(async () => { resolve(places); await Promise.resolve(); });
  expect(view.container.querySelector('section[aria-label="team-one · agents · control/orka"]')).toBeNull();
});
