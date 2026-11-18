import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Send from './Send';

const documentId = '9b2b93c4-375c-4815-9717-39852a96e411';
const id = '00000000-0000-4000-8000-000000000004';
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const target = { id, context_id: id, context_name: 'team-dev', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'orka' };
const provider = { id, name: 'team-model', ready: true };
const place = { kind: 'cluster' as const, target, providers: [provider], providerId: id };
const allowed = { label: 'Access', status: 'allowed', message: 'Access is ready.' };
const check = { documentId, source, targetId: id, providerId: id, model: 'gpt-5.6-terra', rows: [allowed, { label: 'Inference', status: 'unverified', message: 'Checked when tests run.' }], canSend: true, canSeparate: false, action: 'unchanged' };
const failed = { id: 'reads-page', question: 'Read it?', phase: 'Succeeded', answer: 'No page', completedTools: [], assertions: [{ id: 'value', kind: 'contains', expected: 'page', verdict: 'failed' }], verdict: 'failed' };
const result = { documentId, id, targetId: id, providerId: id, targetName: 'team-dev', source, model: 'gpt-5.6-terra', agentName: 'generated-agent', agentAction: 'unchanged', state: 'complete', stage: 'done', acknowledged: [{ kind: 'Agent', name: 'generated-agent', uid: 'uid' }], pending: null, report: { verdict: 'failed', cases: [failed] }, error: null, storageError: false, cancellation: null };
function native(view: unknown = null, preflight: unknown = check, start?: Promise<unknown>, busy: unknown = null) {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => command === 'send_history' ? [] : command === 'send_preflight' ? preflight : command === 'send_start' && start ? start : { documentId, view, busy });
}
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
const card = () => screen.getByRole('region', { name: 'Send and cluster tests' });

test('destination head and one summary precede honest collapsed checks and separate history card', async () => {
  const choose = vi.fn(); native();
  render(<Send place={place} source={source} stale={false} agentName="web-reader" testCount={2} onChooseCluster={choose} />);
  await screen.findByText('Ready to send: 1 check passed');
  expect(within(card()).getByRole('heading', { name: 'To team-dev' })).toBeTruthy();
  expect(within(card()).getByRole('region', { name: 'What you send' }).textContent).toContain('web-reader with 2 tests');
  expect(within(card()).getByRole('region', { name: 'What you send' }).textContent).toContain('Into control/orka on team-dev, namespace agents.');
  const checks = screen.getByText('Ready to send: 1 check passed').closest('details')!;
  expect(checks.open).toBe(false);
  expect(checks.contains(screen.getByText('Checked when tests run.'))).toBe(true);
  expect(screen.getByRole('region', { name: 'Past Sends' }).classList.contains('card')).toBe(true);
  await userEvent.click(screen.getByRole('button', { name: 'Change' })); expect(choose).toHaveBeenCalledOnce();
});
test('capturing a model is not presented as a completed Send after a storage rejection', async () => {
  native({ ...result, state: 'unknown', stage: 'storage', error: 'storage', acknowledged: [], report: null });
  render(<Send place={place} source={source} stale={false} />);
  await screen.findByText('Nothing was sent because Bridge could not save a summary. Check local storage before trying again.');
  expect(card().querySelector('.long-card-summary')?.textContent).toContain('Captured model: GPT-5.6 Terra');
  expect(card().querySelector('.long-card-summary')?.textContent).not.toContain('Sent with');
});
test('only blocking checks are exposed; passing and advisory rows stay folded', async () => {
  native(null, { ...check, canSend: false, action: 'blocked', rows: [...check.rows, { label: 'Missing tool', status: 'blocked', message: 'Install page-reader.' }] });
  render(<Send place={place} source={source} stale={false} />);
  expect((await screen.findByText('Install page-reader.')).closest('details')).toBeNull();
  expect(screen.getByText('Access is ready.').closest('details')?.open).toBe(false);
  expect(screen.getByText('Checked when tests run.').closest('details')?.open).toBe(false);
  expect(screen.getByRole('button', { name: 'Update agent and run tests' }).hasAttribute('disabled')).toBe(true);
});
test.each(['unchanged', 'update'] as const)('current failed result offers editor repair before the precise %s retry', async action => {
  native(result, { ...check, action }); const fix = vi.fn();
  render(<Send place={place} source={source} stale={false} onFixInDesigner={fix} onSeeInOverview={() => {}} />);
  const repair = await screen.findByRole('button', { name: 'Fix in Designer' });
  await waitFor(() => expect(repair.className).toContain('button-primary'));
  expect(screen.getByRole('button', { name: action === 'unchanged' ? 'Run cluster tests again' : 'Update agent and run tests' }).className).toContain('button-secondary');
  await userEvent.click(repair); expect(fix).toHaveBeenCalledOnce();
});
test.each(['Failed', 'CreditLimited'])('%s execution failure keeps retry primary instead of instruction repair', async phase => {
  native({ ...result, report: { verdict: 'failed', cases: [{ ...failed, phase, answer: null, assertions: failed.assertions.map(a => ({ ...a, verdict: 'unknown' })) }] } });
  render(<Send place={place} source={source} stale={false} onFixInDesigner={() => {}} />);
  await screen.findByText('The agent could not finish this test. Try running it again.');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run cluster tests again' }).className).toContain('button-primary'));
  expect(screen.queryByRole('button', { name: 'Fix in Designer' })).toBeNull();
});
test('a known failed assertion still offers repair alongside a separate execution failure', async () => {
  native({ ...result, report: { verdict: 'failed', cases: [failed, { ...failed, id: 'execution', phase: 'Failed', answer: null, assertions: failed.assertions.map(a => ({ ...a, verdict: 'unknown' })) }] } });
  render(<Send place={place} source={source} stale={false} onFixInDesigner={() => {}} />);
  expect((await screen.findByRole('button', { name: 'Fix in Designer' })).className).toContain('button-primary');
});
test('a failed result that finished before confirmed cancellation still offers repair', async () => {
  native({ ...result, cancellation: 'confirmed' });
  render(<Send place={place} source={source} stale={false} onFixInDesigner={() => {}} />);
  expect((await screen.findByRole('button', { name: 'Fix in Designer' })).className).toContain('button-primary');
  expect(screen.getByRole('button', { name: 'Run cluster tests again' }).className).toContain('button-secondary');
});
test.each(['foreign', 'stale', 'status', 'reload', 'check'] as const)('%s result or recovery never promotes Fix in Designer', async mode => {
  native({ ...result, ...(mode === 'foreign' ? { targetId: '00000000-0000-4000-8000-000000000005' } : {}), ...(mode === 'status' ? { cancellation: 'unconfirmed' } : {}) }, { ...check, ...(mode === 'reload' ? { source: { ...source, tests: 'c'.repeat(64) } } : {}), ...(mode === 'check' ? { canSend: false } : {}) });
  render(<Send place={place} source={source} stale={mode === 'stale'} onFixInDesigner={() => {}} onSeeInOverview={() => {}} />);
  await waitFor(() => expect(card().querySelector('[aria-busy="true"]')).toBeNull());
  expect(screen.queryByRole('button', { name: 'Fix in Designer' })).toBeNull();
});
test.each(['running', 'foreign'] as const)('%s Send keeps passed checks neutral while a new Send is unavailable', async mode => {
  native(mode === 'running' ? { ...result, state: 'running', stage: 'observe', report: null } : null, check, undefined,
    mode === 'foreign' ? { documentId: '00000000-0000-4000-8000-000000000005', name: 'another-agent' } : null);
  render(<Send place={place} source={source} stale={false} />);
  await screen.findByText(mode === 'running' ? 'Waiting for answers and evaluating tests' : 'another-agent is sending tests.');
  expect(screen.queryByText(/^Ready to send:/)).toBeNull();
  expect(screen.getByText('1 check passed')).toBeTruthy();
});
test('Change is disabled with a reason during admission, before a running snapshot arrives', async () => {
  native(null, check, new Promise(() => {}));
  render(<Send place={place} source={source} stale={false} onChooseCluster={() => {}} />);
  const send = await screen.findByRole('button', { name: 'Update agent and run tests' });
  await waitFor(() => expect(send.hasAttribute('disabled')).toBe(false));
  await userEvent.click(send);
  const change = screen.getByRole('button', { name: 'Change' });
  expect(change.hasAttribute('disabled')).toBe(true);
  expect(change.title).toContain('Wait');
  expect(screen.queryByText(/^Ready to send:/)).toBeNull();
  expect(screen.getByText('1 check passed')).toBeTruthy();
});
