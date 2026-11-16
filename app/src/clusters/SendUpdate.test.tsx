import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import { SAMPLE_DOCUMENT_ID } from '../designer/agent-api';
import Send from './Send';
import { sendPreflight } from './send-api';

const id = '00000000-0000-4000-8000-000000000004';
const target = { id, context_id: id, context_name: 'team-dev', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'orka' };
const provider = { id, name: 'team-model', ready: true };
const place = { kind: 'cluster' as const, target, providers: [provider], providerId: id };
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const preflight = { documentId: SAMPLE_DOCUMENT_ID, source, targetId: id, providerId: id, model: 'model', rows: [{ label: 'Access', status: 'allowed', message: 'Ready' }], canSend: true, action: 'create', canSeparate: false };
const active = { documentId: SAMPLE_DOCUMENT_ID, id, targetId: id, providerId: id, targetName: 'team-dev', source, model: 'model', agentName: 'bridge-run-frozen', state: 'running', stage: 'observe', acknowledged: [], pending: null, report: null, error: null, storageError: false, cancellation: null };
function native(check: unknown = preflight, initial: unknown = null) {
  vi.stubGlobal('isTauri', true);
  let view = initial; let busy: unknown = null;
  const ipc = vi.fn<(command: string, args?: unknown) => unknown>((command) => {
    if (command === 'send_history') return [];
    if (command === 'send_preflight') return check;
    if (command === 'send_start') view = active;
    if (command === 'send_start' || command === 'send_status') return { documentId: SAMPLE_DOCUMENT_ID, view, busy };
    throw Error('PRIVATE_DIAGNOSTIC');
  });
  mockIPC(ipc);
  return { ipc, setCheck(value: unknown) { check = value; }, setView(value: unknown) { view = value; }, setBusy(value: unknown) { busy = value; } };
}
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
const panel = (blocked?: string) => <Send place={place} source={source} stale={false} blocked={blocked} onSeeInOverview={() => undefined} />;

test.each([
  ['create', 'Send and run tests'],
  ['update', 'Update agent and run tests'],
  ['unchanged', 'Update agent and run tests'],
])('%s action follows native effective-spec decision', async (action, label) => {
  const { ipc } = native({ ...preflight, action, canSeparate: action !== 'create' });
  render(panel());
  await waitFor(() => expect((screen.getByRole('button', { name: label }) as HTMLButtonElement).disabled).toBe(false));
  expect(screen.queryByText(/Each Send creates a new agent/)).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: label }));
  const calls = ipc.mock.calls.filter(([cmd]) => cmd === 'send_start');
  expect(calls).toHaveLength(1);
  expect(calls[0][1]).toMatchObject({ documentId: SAMPLE_DOCUMENT_ID, targetId: id, providerId: id, expectedSource: source });
  expect((calls[0][1] as Record<string, unknown>).separateAgent).not.toBe(true);
});

test('an older Orka blocks normal update but permits only an explicit separate-agent Send', async () => {
  const explanation = "This Orka can't safely update this agent";
  const { ipc } = native({ ...preflight, action: 'blocked', canSend: false, canSeparate: true, rows: [{ label: 'Agent update', status: 'blocked', message: explanation }] });
  render(panel());
  expect(await screen.findByText(explanation)).toBeTruthy();
  const separate = await screen.findByRole('button', { name: 'Send as a separate agent' });
  expect((separate as HTMLButtonElement).disabled).toBe(false);
  const actions = screen.getByRole('button', { name: 'Update agent and run tests' }).parentElement!;
  expect(separate.parentElement).toBe(actions);
  expect(actions.querySelectorAll('.button-primary')).toHaveLength(1);
  expect(actions.querySelectorAll('.button-secondary')).toHaveLength(2);
  expect(screen.getByText('Creates a new agent here. Previous work may still run; this does not stop or replace it.')).toBeTruthy();
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_start')).toBe(false);
  await userEvent.click(separate);
  expect(ipc.mock.calls.find(([cmd]) => cmd === 'send_start')?.[1]).toMatchObject({ separateAgent: true, expectedSource: source });
});

test('separate-agent escape cannot bypass a draft/source block', async () => {
  const { ipc } = native({ ...preflight, action: 'blocked', canSend: false, canSeparate: true });
  const view = render(panel());
  await screen.findByRole('button', { name: 'Send as a separate agent' });
  view.rerender(panel('Save your changes first'));
  expect(screen.queryByRole('button', { name: 'Send as a separate agent' })).toBeNull();
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_start')).toBe(false);
});

test('separate-agent escape cannot bypass a changed disk snapshot', async () => {
  native({ ...preflight, action: 'blocked', canSend: false, canSeparate: true, source: { ...source, tests: 'c'.repeat(64) } });
  render(panel());
  await screen.findByRole('button', { name: 'Reload saved files' });
  expect(screen.queryByRole('button', { name: 'Send as a separate agent' })).toBeNull();
});

test('a finished Send refreshes its native binding decision before another click', async () => {
  const state = native();
  render(panel());
  await waitFor(() => expect((screen.getByRole('button', { name: 'Send and run tests' }) as HTMLButtonElement).disabled).toBe(false));
  await userEvent.click(screen.getByRole('button', { name: 'Send and run tests' }));
  await screen.findByRole('button', { name: 'Cancel tests' });
  state.setCheck({ ...preflight, action: 'unchanged', canSeparate: true });
  state.setView({ ...active, acknowledged: [{ kind: 'Agent', name: 'bridge-run-frozen', uid: 'uid-agent' }], state: 'complete', stage: 'done', report: { verdict: 'passed', cases: [{ id: 'test', question: 'Question', phase: 'Succeeded', answer: 'Answer', completedTools: [], assertions: [{ id: 'check', kind: 'contains', expected: 'Answer', verdict: 'passed' }], verdict: 'passed' }] } });
  await waitFor(() => expect((screen.getByRole('button', { name: 'Run cluster tests again' }) as HTMLButtonElement).disabled).toBe(false));
});

test('another document finishing refreshes preflight even with no Send in this document', async () => {
  const state = native({ ...preflight, action: 'blocked', canSend: false, rows: [{ label: 'Earlier Send', status: 'blocked', message: 'Another Send blocks updates' }] });
  state.setBusy({ documentId: '00000000-0000-4000-8000-000000000005', name: 'other-agent' });
  render(panel());
  await screen.findByText('Another Send blocks updates');
  state.setCheck(preflight); state.setBusy(null);
  await waitFor(() => expect((screen.getByRole('button', { name: 'Send and run tests' }) as HTMLButtonElement).disabled).toBe(false));
  expect(state.ipc.mock.calls.filter(([cmd]) => cmd === 'send_preflight').length).toBeGreaterThan(1);
  expect(state.ipc.mock.calls.some(([cmd]) => cmd === 'send_start')).toBe(false);
});

test('read-only Check again refreshes a recovered binding without submitting work', async () => {
  const state = native({ ...preflight, action: 'blocked', canSend: false, canSeparate: true, rows: [{ label: 'Earlier Send', status: 'blocked', message: 'Earlier work is not confirmed.' }] });
  render(panel());
  await screen.findByText('Earlier work is not confirmed.');
  state.setCheck({ ...preflight, action: 'unchanged', canSeparate: true });
  await userEvent.click(screen.getByRole('button', { name: 'Check again' }));
  await waitFor(() => expect((screen.getByRole('button', { name: 'Update agent and run tests' }) as HTMLButtonElement).disabled).toBe(false));
  expect(state.ipc.mock.calls.some(([cmd]) => cmd === 'send_start')).toBe(false);
});

test('an acknowledged update is separate from an unknown test outcome', async () => {
  native({ ...preflight, action: 'unchanged', canSeparate: true }, { ...active, agentAction: 'update', state: 'unknown', stage: 'observe', error: 'observe', acknowledged: [{ kind: 'Agent', name: active.agentName, uid: 'uid-agent' }] });
  render(panel());
  expect(await screen.findByText('Agent updated on the cluster.')).toBeTruthy();
  expect(screen.getByText('The result could not be confirmed. The cluster may still be running checks.')).toBeTruthy();
});

test('an unacknowledged update never claims that the agent was updated', async () => {
  native(preflight, { ...active, agentAction: 'update', state: 'unknown', stage: 'updateUnconfirmed', error: 'updateUnconfirmed' });
  render(panel());
  await screen.findByText('Outcome not confirmed. Check this cluster before trying again. Nothing was retried.');
  expect(screen.queryByText('Agent updated on the cluster.')).toBeNull();
});

test.each([
  ['submitRejected', 'Orka did not accept the resource. Check cluster access and try again.'],
  ['updateConflict', 'This agent changed outside Bridge. Your files are unchanged.'],
  ['updateUnsupported', "This Orka can't safely update this agent"],
  ['updateMissing', 'The linked agent is no longer available on this cluster. Your files are unchanged.'],
  ['updateDenied', 'Cannot update this agent with this cluster identity.'],
  ['updateInvalid', 'Orka did not accept the agent update. Your files are unchanged.'],
  ['updateUnconfirmed', 'Outcome not confirmed. Check this cluster before trying again. Nothing was retried.'],
  ['earlierWork', 'An earlier Send is not finished or its outcome is not confirmed. Check its status or cancel its tests before updating this agent.'],
])('%s is a distinct native outcome, never raw diagnostics', async (error, message) => {
  const { ipc } = native({ ...preflight, action: 'blocked', canSend: false, canSeparate: true }, { ...active, state: 'unknown', stage: error, error });
  render(panel());
  expect(await screen.findByText(message)).toBeTruthy();
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_start')).toBe(false);
  expect(document.body.textContent).not.toContain('PRIVATE_DIAGNOSTIC');
});

test.each([
  { action: 'overwrite' }, { canSeparate: 'yes' }, { action: 'blocked', canSend: true },
])('malformed native deployment decision fails closed: %j', async invalid => {
  native({ ...preflight, ...invalid });
  await expect(sendPreflight(id, id)).rejects.toThrow('Destination checks could not be read');
});
