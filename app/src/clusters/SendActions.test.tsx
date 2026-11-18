import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Send, { SendHistory } from './Send';

const documentId = '9b2b93c4-375c-4815-9717-39852a96e411';
const id = '00000000-0000-4000-8000-000000000004';
const target = { id, context_id: id, context_name: 'team-dev', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'orka' };
const provider = { id, name: 'team-model', ready: true };
const place = { kind: 'cluster' as const, target, providers: [provider], providerId: id };
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const check = { documentId, source, targetId: id, providerId: id, model: 'gpt-5.6-terra', rows: [{ label: 'Access', status: 'allowed', message: 'Ready' }], canSend: true, canSeparate: true, action: 'unchanged' };
const row = { id: 'reads-page', title: 'Reads a page', question: 'Read it?', phase: 'Succeeded', answer: 'Saved answer', completedTools: [], assertions: [{ id: 'value', kind: 'contains', expected: 'page', verdict: 'failed' }], verdict: 'failed' };
const result = { documentId, id, targetId: id, providerId: id, targetName: 'team-dev', source, model: 'gpt-5.6-terra', agentName: 'generated-agent', agentAction: 'unchanged', state: 'complete', stage: 'done', acknowledged: [{ kind: 'Agent', name: 'generated-agent', uid: 'uid' }], pending: null, report: { verdict: 'failed', cases: [row] }, error: null, storageError: false, cancellation: null };
function native(view: unknown = result, preflight: unknown = check) {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn(command => command === 'send_history' ? [] : command === 'send_preflight' ? preflight : command === 'send_history_check' || command === 'send_history_cancel' ? {} : { documentId, view, busy: null });
  mockIPC(ipc); return ipc;
}
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
const card = () => screen.getByRole('region', { name: 'Send and cluster tests' });
function actionRow() { return card().querySelector<HTMLElement>('.workspace-step-actions')!; }
async function openDetails() { await userEvent.click(within(card()).getByText('Resources and file hashes')); }

test.each(['failed', 'cancelled', 'unknown', 'passed'] as const)('%s result leads with the useful next action and only one secondary', async verdict => {
  const seen = vi.fn(); native({ ...result, report: { verdict, cases: [{ ...row, verdict, ...(verdict === 'cancelled' ? { phase: 'Cancelled', assertions: [] } : {}) }] } });
  render(<Send place={place} source={source} stale={false} onSeeInOverview={seen} />);
  await screen.findByText(/Ready to send: \d+ checks? passed/);
  const buttons = within(actionRow()).getAllByRole('button');
  expect(buttons.map(b => b.textContent)).toEqual(verdict === 'passed' ? ['See in Overview', 'Run cluster tests again'] : ['Run cluster tests again', 'See in Overview']);
  expect(buttons[0].className).toContain('button-primary');
  expect(buttons[1].className).toContain('button-secondary');
  expect(screen.getByRole('button', { name: 'Send as a separate agent' }).closest('details')?.open).toBe(false);
  await openDetails();
  expect(screen.getByRole('button', { name: 'Send as a separate agent' })).toBeTruthy();
  await userEvent.click(screen.getByRole('button', { name: 'See in Overview' }));
  expect(seen).toHaveBeenCalledWith(id);
});

test.each(['requested', 'unconfirmed', 'denied', 'unsupported', 'confirmed'] as const)('%s cancellation leads with status of the recorded attempt, not Overview or a new Send', async cancellation => {
  const ipc = native({ ...result, state: 'unknown', cancellation, report: { verdict: 'unknown', cases: [row] } });
  render(<Send place={place} source={source} stale={false} onSeeInOverview={() => {}} />);
  await screen.findByText('Saved answer');
  const buttons = within(actionRow()).getAllByRole('button');
  expect(buttons[0].textContent).toBe('Check cluster status');
  expect(buttons[0].className).toContain('button-primary');
  expect(buttons.map(b => b.textContent)).toEqual(['unsupported', 'confirmed'].includes(cancellation) ? ['Check cluster status'] : ['Check cluster status', 'Retry cancellation']);
  await openDetails();
  if (['unsupported', 'confirmed'].includes(cancellation)) expect(screen.queryByRole('button', { name: 'Retry cancellation' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Run cluster tests again' })).toBeTruthy();
  await userEvent.click(buttons[0]);
  await waitFor(() => expect(ipc).toHaveBeenCalledWith('send_history_check', expect.objectContaining({ sendId: id })));
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_start' || cmd === 'send_history_cancel')).toBe(false);
});

test('saved unsupported cancellation cannot be retried even when unfinished resources allow canCancel', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => command === 'send_history' ? [{ id, source, targetDisplay: 'team-dev', targetUids: { serviceNamespace: 'control', serviceUid: 's', deploymentNamespace: 'control', deploymentUid: 'd', watchedNamespace: 'agents' }, provider: 'team-model', plannedAgentName: 'agent', agentUid: 'uid', verdict: 'unknown', time: 1 }] : { id, documentId, model: 'model', capturedAt: 1, report: { verdict: 'unknown', cases: [row] }, interrupted: true, canCheck: true, canRemove: false, canCancel: true, cancellation: 'unsupported', detailState: 'saved', availability: null });
  render(<SendHistory revision="same" />);
  await userEvent.click(await screen.findByText('Show saved details'));
  await screen.findByText('This Orka cannot cancel tests. Work may still be running.');
  expect(screen.queryByRole('button', { name: 'Retry cancellation' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Check cluster status' })).toBeTruthy();
  expect(screen.getByText('Reads a page')).toBeTruthy();
});

test('a file mismatch makes Reload the primary and preserves its handler', async () => {
  const reload = vi.fn().mockResolvedValue(undefined); native(result, { ...check, source: { ...source, tests: 'c'.repeat(64) } });
  render(<Send place={place} source={source} stale={false} onReloadSource={reload} onSeeInOverview={() => {}} />);
  const button = await screen.findByRole('button', { name: 'Reload saved files' });
  expect(button.className).toContain('button-primary');
  expect(within(actionRow()).getAllByRole('button')).toHaveLength(2);
  await userEvent.click(button); expect(reload).toHaveBeenCalledOnce();
});

test('an earlier target is labelled and preflight failure offers a check, not Overview as primary', async () => {
  native({ ...result, targetName: 'earlier-team', targetId: '00000000-0000-4000-8000-000000000005' }, { ...check, canSend: false, action: 'blocked' });
  render(<Send place={place} source={source} stale={false} onSeeInOverview={() => {}} />);
  const button = await screen.findByRole('button', { name: 'Check again' });
  expect(button.className).toContain('button-primary');
  expect(within(actionRow()).getAllByRole('button')).toHaveLength(2);
  expect(card().querySelector('.long-card-summary')?.textContent).toContain('Earlier result on earlier-team');
  expect(within(card()).getByText('Destination').parentElement?.textContent).toContain('team-dev');
});

test('returning to an unchanged destination uses its preflight rerun wording, not the retained result target', async () => {
  native({ ...result, targetId: '00000000-0000-4000-8000-000000000005', targetName: 'earlier-team' });
  render(<Send place={place} source={source} stale={false} onSeeInOverview={() => {}} />);
  await screen.findByText(/Ready to send: \d+ checks? passed/);
  const button = screen.getByRole('button', { name: 'Run cluster tests again' });
  expect(button.className).toContain('button-primary');
  expect(screen.queryByRole('button', { name: 'Update agent and run tests' })).toBeNull();
});

test('status recovery keeps its prerequisite cluster selection in Details when no destination is selected', async () => {
  const choose = vi.fn(); native({ ...result, state: 'unknown', cancellation: 'unsupported', report: { verdict: 'unknown', cases: [row] } });
  render(<Send place={{ kind: 'thisComputer' }} source={source} stale={false} onChooseCluster={choose} />);
  await screen.findByRole('button', { name: 'Check cluster status' });
  const button = screen.getByRole('button', { name: 'Choose a cluster' });
  expect(button.closest('details')?.open).toBe(false);
  await openDetails(); await userEvent.click(button);
  expect(choose).toHaveBeenCalledOnce();
  expect(within(actionRow()).getAllByRole('button').map(b => b.textContent)).toEqual(['Check cluster status']);
});

test('Send copy is concise and friendly while recorded raw model/provider remain in Details', async () => {
  native(); render(<Send place={place} source={source} stale={false} />);
  await screen.findByText(/Ready to send: \d+ checks? passed/);
  expect(within(card()).getByRole('region', { name: 'What you send' }).textContent).toContain('GPT-5.6 Terra, checked when tests run');
  expect(screen.queryByText('Team clusters are early in this preview.')).toBeNull();
  const separateHelp = screen.getByText('Sends as a new agent; the old one keeps running.');
  expect(separateHelp.closest('details')?.open).toBe(false);
  expect(within(card()).getByRole('region', { name: 'What you send' }).contains(separateHelp)).toBe(false);
  expect(screen.queryByText('Using the same agent without changing its configuration.')).toBeNull();
  await openDetails();
  expect(within(card()).getByText('Model sent').parentElement?.textContent).toContain('gpt-5.6-terra');
  expect(within(card()).getByText('team-model')).toBeTruthy();
});
