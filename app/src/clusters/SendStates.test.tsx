import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Send from './Send';

const id = '9b2b93c4-375c-4815-9717-39852a96e411';
const sendId = '00000000-0000-4000-8000-000000000004';
const target = { id: '00000000-0000-4000-8000-000000000002', context_id: '00000000-0000-4000-8000-000000000001', context_name: 'team-dev', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'ghcr.io/orka:0.9.4' };
const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'team-model', ready: true };
const place = { kind: 'cluster' as const, target, providers: [provider], providerId: provider.id };
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const allowed = { documentId: id, source, targetId: target.id, providerId: provider.id, model: 'literal-model', rows: [{ label: 'AI connection', status: 'allowed', message: 'Ready.' }], canSend: true, action: 'create', canSeparate: false };
const row = (verdict: string) => ({ id: 'reads-the-title', question: 'What is the title?', phase: 'Succeeded', answer: 'The title', completedTools: [], assertions: [{ id: 'has', kind: 'contains', expected: 'Title', verdict }], verdict });
const view = (state: string, report: unknown, extra: object = {}) => ({ documentId: id, id: sendId, targetId: target.id, providerId: provider.id, targetName: target.context_name, source, model: 'literal-model', agentName: 'bridge-run-one', state, stage: state === 'running' ? 'observe' : 'done', acknowledged: [{ kind: 'Agent', name: 'bridge-run-one', uid: 'agent-uid' }], pending: null, report, error: null, storageError: false, agentAction: 'create', ...extra });
const finished = (verdict: 'passed' | 'failed') => view('complete', { verdict, cases: [row(verdict)] });
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
function native(current: unknown = null, check: unknown = allowed) {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => command === 'send_status' ? { documentId: id, view: current, busy: null } : command === 'send_history' ? [] : command === 'send_preflight' ? check : null);
}
const card = () => screen.getByRole('region', { name: 'Send and cluster tests' });
test('a local freshness rejection is respected despite matching cached hashes', async () => {
  native(); render(<Send place={place} source={source} stale={false} local={{ state: 'complete', canRun: true, source, sourceCurrent: false, report: { verdict: 'passed', cases: [] }, acknowledged: [], pending: null, error: null }} />);
  await screen.findByText('Destination ready');
  expect(screen.queryByText('Passed on these files')).toBeNull();
  expect(screen.getByText('Earlier files')).toBeTruthy();
});
test('a ready destination reads Ready to send with its checks collapsed behind one line', async () => {
  native(); render(<Send place={place} source={source} stale={false} />);
  const toggle = await screen.findByText('Destination ready');
  expect(screen.getByText('Ready to send')).toBeTruthy();
  const checks = toggle.closest('details')!;
  expect(checks.open).toBe(false);
  expect(within(checks).getByText('Show checks')).toBeTruthy();
  expect(within(card()).queryByRole('region', { name: 'What happened' })).toBeNull();
  expect(screen.queryByText('Passed')).toBeNull();
  expect(screen.getByText(/Repeated Sends/).textContent).toContain('fails. Sending');
});
test('blocked and unknown checks stay inline with their reason and mark the pill Needs attention', async () => {
  native(null, { ...allowed, canSend: false, action: 'blocked', rows: [{ label: 'page-reader', status: 'blocked', message: 'Create page-reader on team-dev.' }] });
  render(<Send place={place} source={source} stale={false} />);
  const reason = await screen.findByText('Create page-reader on team-dev.');
  expect(reason.closest('details')).toBeNull();
  expect(screen.getByText('Needs attention')).toBeTruthy();
});
test('advisory unverified rows in a sendable preflight stay with the collapsed checks, as the native check reports them', async () => {
  native(null, { ...allowed, rows: [{ label: 'Agent access', status: 'allowed', message: 'You can create agents.' }, { label: 'Model inference', status: 'unverified', message: 'Inference is checked when tests run.' }] });
  render(<Send place={place} source={source} stale={false} />);
  const checks = (await screen.findByText('Destination ready')).closest('details')!;
  expect(checks.open).toBe(false);
  expect(checks.contains(screen.getByText('Inference is checked when tests run.'))).toBe(true);
  expect(screen.getByText('Ready to send')).toBeTruthy();
  expect(screen.queryByRole('region', { name: 'Before Send' })).toBeNull();
});
test('the first and a later Send have fixed action wording; Run cluster tests again is only a result rerun', async () => {
  for (const [action, label] of [['create', 'Send and run tests'], ['update', 'Update agent and run tests'], ['unchanged', 'Update agent and run tests']] as const) {
    native(null, { ...allowed, action }); const run = render(<Send place={place} source={source} stale={false} />);
    await screen.findByRole('button', { name: label });
    expect(screen.queryByRole('button', { name: 'Run cluster tests again' })).toBeNull();
    run.unmount();
  }
});
test('a failed result leads with its verdict, keeps the fixed section order and puts the collapsed checks after the answer', async () => {
  native(view('complete', { verdict: 'failed', cases: [row('failed')] }, { agentAction: 'update' }), { ...allowed, action: 'unchanged' });
  render(<Send place={place} source={source} stale={false} onSeeInOverview={() => undefined} />);
  expect(await screen.findByRole('heading', { name: '0 of 1 tests passed', level: 3 })).toBeTruthy();
  expect(screen.getByText('Failed', { selector: '.pill' })).toBeTruthy();
  const order = ['What you send', 'What happened'].map(name => within(card()).getByRole('region', { name }));
  expect(order[0].compareDocumentPosition(order[1]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  const checks = (await screen.findByText('Destination ready')).closest('details')!;
  expect(order[1].compareDocumentPosition(checks) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  const details = [...card().querySelectorAll('.card-details')].at(-1)!;
  expect(checks.compareDocumentPosition(details) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});
test('running keeps the destination fixed with Cancel tests, partial answers and no second Send', async () => {
  native(view('running', { verdict: 'unknown', cases: [row('passed')] }));
  render(<Send place={place} source={source} stale={false} />);
  expect(await screen.findByText('Running tests')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Cancel tests' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: /Send and run tests|Update agent and run tests|Run cluster tests again/ })).toBeNull();
  expect(screen.getByText('The title')).toBeTruthy();
});
test('an acknowledged agent makes See in Overview the one primary, and the rerun secondary', async () => {
  const seen = vi.fn(); const user = userEvent.setup();
  native(finished('failed'), { ...allowed, action: 'unchanged' });
  render(<Send place={place} source={source} stale={false} onSeeInOverview={seen} />);
  const open = await screen.findByRole('button', { name: 'See in Overview' });
  expect(open.className).toContain('primary');
  const rerun = screen.getByRole('button', { name: 'Run cluster tests again' });
  expect(rerun.className).toContain('secondary');
  expect(card().querySelectorAll('.button-primary').length).toBe(1);
  await user.click(open);
  expect(seen).toHaveBeenCalledWith(sendId);
});
test('an earlier result from another destination never labels a first Send there as a rerun', async () => {
  native(finished('passed'));
  const other = { ...target, id: '00000000-0000-4000-8000-000000000005', context_name: 'other-team' };
  const calls = vi.fn((command: string) => command === 'send_status' ? { documentId: id, view: finished('passed'), busy: null } : command === 'send_history' ? [] : command === 'send_preflight' ? { ...allowed, targetId: other.id } : null);
  mockIPC(calls);
  render(<Send place={{ ...place, target: other }} source={source} stale={false} onSeeInOverview={() => undefined} />);
  await screen.findByText('Earlier result', { selector: '.pill' });
  const send = await screen.findByRole('button', { name: 'Send and run tests' });
  await waitFor(() => expect((send as HTMLButtonElement).disabled).toBe(false));
  expect(screen.queryByRole('button', { name: 'Run cluster tests again' })).toBeNull();
  expect(screen.getByRole('button', { name: 'See in Overview' })).toBeTruthy();
});
test('without an acknowledged agent there is no See in Overview link', async () => {
  native(view('unknown', null, { acknowledged: [], stage: 'submit', error: 'submit' }));
  render(<Send place={place} source={source} stale={false} onSeeInOverview={() => undefined} />);
  await screen.findByText('Sending was not confirmed. Named resources may still exist. Nothing was retried.');
  expect(screen.queryByRole('button', { name: 'See in Overview' })).toBeNull();
});
test('earlier or unconfirmed files never show a green Passed pill', async () => {
  native(finished('passed'));
  const { rerender } = render(<Send place={place} source={{ ...source, tests: 'c'.repeat(64) }} stale={false} />);
  expect(await screen.findByText('Earlier files', { selector: '.pill' })).toBeTruthy();
  expect(screen.queryByText('Passed', { selector: '.pill' })).toBeNull();
  clearMocks(); native(finished('passed'), { ...allowed, source: null, canSend: false, rows: [{ label: 'Saved files', status: 'cannotCheck', message: 'Your saved files could not be read.' }] });
  rerender(<Send place={{ ...place }} source={source} stale={false} />);
  await waitFor(() => expect(screen.getByText('Revision not confirmed', { selector: '.pill' })).toBeTruthy());
  expect(screen.queryByText('Passed', { selector: '.pill' })).toBeNull();
});
test.each([['updateConflict', 'Needs attention'], ['updateUnconfirmed', 'Result unknown']])('an %s update uses its known or uncertain pill state', async (error, word) => {
  native(view('unknown', null, { agentAction: 'update', stage: error, error }), { ...allowed, action: 'update' });
  render(<Send place={place} source={source} stale={false} onSeeInOverview={() => undefined} />);
  expect(await screen.findByText(word, { selector: '.pill' })).toBeTruthy();
});
test('Send stays on screen without a selected cluster and offers the way to choose one', async () => {
  native(); render(<Send place={{ kind: 'thisComputer' }} source={source} stale={false} onChooseCluster={() => undefined} />);
  expect(card().hasAttribute('hidden')).toBe(false);
  expect(await screen.findByRole('button', { name: 'Choose a cluster' })).toBeTruthy();
});
