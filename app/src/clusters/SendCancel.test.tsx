import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import { SAMPLE_DOCUMENT_ID } from '../designer/agent-api';
import Send, { SendHistory } from './Send';
import { sendCall, sendHistoryDetail, type SendView } from './send-api';

const id = '00000000-0000-4000-8000-000000000004';
const target = { id, context_id: id, context_name: 'team-dev', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'orka' };
const provider = { id, name: 'team-model', ready: true };
const place = { kind: 'cluster' as const, target, providers: [provider], providerId: id };
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const passed = { id: 'finished', question: 'First question', phase: 'Succeeded', answer: 'Completed answer kept', completedTools: [], assertions: [{ id: 'value', kind: 'contains' as const, expected: 'kept', verdict: 'passed' as const }], verdict: 'passed' as const };
const report = { verdict: 'cancelled' as const, cases: [passed, { ...passed, id: 'cancelled', phase: 'Cancelled', answer: null, verdict: 'cancelled' as const, assertions: [] }, { ...passed, id: 'not-submitted', phase: 'NotRun', answer: null, verdict: 'unknown' as const, assertions: [{ ...passed.assertions[0], verdict: 'unknown' as const }] }] };
const active = { documentId: SAMPLE_DOCUMENT_ID, id, targetId: id, providerId: id, targetName: 'team-dev', source, model: 'model', agentName: 'bridge-run-frozen', state: 'running', stage: 'observe', acknowledged: [], pending: null, report: null, error: null, storageError: false, cancellation: null };
const preflight = { documentId: SAMPLE_DOCUMENT_ID, source, targetId: id, providerId: id, model: 'model', rows: [{ label: 'Access', status: 'allowed', message: 'Ready' }], canSend: true };
function native(view: unknown = active) {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn((command: string) => {
    if (command === 'send_history') return [];
    if (command === 'send_preflight') return preflight;
    if (command === 'send_cancel') view = { ...active, cancellation: 'cancelling' };
    if (command === 'send_status' || command === 'send_cancel') return { documentId: SAMPLE_DOCUMENT_ID, view, busy: null };
    throw Error('PRIVATE_DIAGNOSTIC');
  });
  mockIPC(ipc); return ipc;
}
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
function panel() { return <Send place={place} source={source} stale={false} visible remote />; }

test('Cancel tests uses explicit cancellation IPC, never Stop, and promptly announces progress', async () => {
  const ipc = native(); render(panel());
  await userEvent.click(await screen.findByRole('button', { name: 'Cancel tests' }));
  expect(await screen.findByText('Cancelling tests…', { selector: 'span' })).toBeTruthy();
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'send_cancel')).toHaveLength(1);
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_stop')).toBe(false);
  expect((screen.getByRole('button', { name: 'Cancel tests' }) as HTMLButtonElement).disabled).toBe(true);
});

test.each([
  ['requested', 'Cancellation requested. Waiting for the cluster.'],
  ['unconfirmed', 'Cancellation not confirmed. Work may still be running.'],
  ['denied', 'Cannot cancel with this cluster identity'],
  ['unsupported', 'This Orka cannot cancel tests. Work may still be running.'],
] as const)('%s cancellation retains answers and offers explicit status and retry without gating Send', async (cancellation, message) => {
  const ipc = native({ ...active, state: 'unknown', stage: 'stopped', report: { verdict: 'unknown', cases: [passed] }, cancellation });
  render(panel());
  expect(await screen.findByText(message)).toBeTruthy();
  expect(screen.getByText('Completed answer kept')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Check cluster status' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Retry cancellation' })).toBeTruthy();
  await waitFor(() => expect((screen.getByRole('button', { name: 'Send and run tests' }) as HTMLButtonElement).disabled).toBe(false));
  expect(screen.queryByText('Stopped watching. The cluster may still be running checks.')).toBeNull();
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_history_cancel')).toBe(false);
});

test('confirmed cancellation renders cancelled and Not run separately, keeping completed answers', async () => {
  native({ ...active, state: 'complete', stage: 'done', report, cancellation: 'confirmed' }); render(panel());
  expect(await screen.findByText('Tests cancelled on the cluster. Completed answers kept.')).toBeTruthy();
  expect(screen.getByText('Not run')).toBeTruthy();
  expect(screen.getByText('Completed answer kept')).toBeTruthy();
  const cancelled = screen.getByText('cancelled', { selector: 'code' }).closest('details')!;
  expect(cancelled.textContent).toContain('Cancelled');
  expect(cancelled.textContent).not.toContain('Not met');
  expect(cancelled.querySelector('.status-failed, .status-passed')).toBeNull();
  expect(screen.queryByText(/Stopped watching/)).toBeNull();
});

test.each([
  null,
  { verdict: 'unknown', cases: [passed] },
  { verdict: 'unknown', cases: [{ ...passed, phase: 'NotRun', answer: null, verdict: 'unknown' }] },
])('terminal receipts with incomplete results never invent an outcome (%j)', async incomplete => {
  native({ ...active, state: 'unknown', stage: 'stopped', report: incomplete, cancellation: 'confirmed' }); render(panel());
  expect(await screen.findByText('Test results could not be confirmed. Check cluster status.')).toBeTruthy();
  expect(screen.queryByText('Stopped watching')).toBeNull();
  expect(screen.queryByText('No tests were submitted.')).toBeNull();
  expect(screen.queryByText('Tests finished before cancellation.')).toBeNull();
  expect(screen.getByRole('button', { name: 'Check cluster status' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Retry cancellation' })).toBeNull();
});

test('a completed race retains its passing result without claiming tests were cancelled', async () => {
  native({ ...active, state: 'complete', stage: 'done', report: { verdict: 'passed', cases: [passed] }, cancellation: 'confirmed' }); render(panel());
  expect(await screen.findByText('Tests finished before cancellation.')).toBeTruthy();
  expect(screen.queryByText('Tests cancelled on the cluster. Completed answers kept.')).toBeNull();
  expect(screen.getByText('1 of 1 passed')).toBeTruthy();
});

test('saved cancellation is explicit and retains answers when a retry fails', async () => {
  vi.stubGlobal('isTauri', true);
  const detail = { id, documentId: SAMPLE_DOCUMENT_ID, model: 'model', capturedAt: 1, report: { verdict: 'unknown', cases: [passed] }, interrupted: true, canCheck: true, canRemove: false, canCancel: true, cancellation: 'unconfirmed', detailState: 'saved', availability: null };
  const ipc = vi.fn((command: string) => {
    if (command === 'send_history') return [{ id, source, targetDisplay: 'team-dev', targetUids: { serviceNamespace: 'control', serviceUid: 's', deploymentNamespace: 'control', deploymentUid: 'd', watchedNamespace: 'agents' }, provider: 'team-model', plannedAgentName: 'agent', agentUid: 'uid', verdict: 'unknown', time: 1 }];
    if (command === 'send_history_details') return detail;
    throw Error('PRIVATE_DIAGNOSTIC');
  }); mockIPC(ipc);
  render(<SendHistory revision="same" />);
  await userEvent.click(await screen.findByText('Show saved details'));
  expect(await screen.findByText('Cancellation not confirmed. Work may still be running.')).toBeTruthy();
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_history_cancel')).toBe(false);
  await userEvent.click(screen.getByRole('button', { name: 'Retry cancellation' }));
  expect(await screen.findByText(/Cancellation could not be confirmed/)).toBeTruthy();
  expect(screen.getByText('Completed answer kept')).toBeTruthy();
  expect(document.body.textContent).not.toContain('PRIVATE_DIAGNOSTIC');
});

test.each(['outcome', 'answer'])('successful control refreshes an already-open saved %s', async change => {
  vi.stubGlobal('isTauri', true);
  let cancellation = 'requested'; let recovered = false;
  const detail = () => ({ id, documentId: SAMPLE_DOCUMENT_ID, model: 'model', capturedAt: 1, report: { verdict: 'unknown', cases: recovered ? [passed, { ...passed, id: 'newly-finished' }] : [passed] }, interrupted: true, canCheck: true, canRemove: false, canCancel: true, cancellation, detailState: 'saved', availability: null });
  mockIPC(command => {
    if (command === 'send_history') return [{ id, source, targetDisplay: 'team-dev', targetUids: { serviceNamespace: 'control', serviceUid: 's', deploymentNamespace: 'control', deploymentUid: 'd', watchedNamespace: 'agents' }, provider: 'team-model', plannedAgentName: 'agent', agentUid: 'uid', verdict: 'unknown', time: 1 }];
    if (command === 'send_preflight') return preflight;
    if (command === 'send_history_details') return detail();
    if (command === 'send_history_cancel' || command === 'send_history_check') {
      if (change === 'outcome') cancellation = 'denied'; else recovered = true;
      return detail();
    }
    if (command === 'send_status') return { documentId: SAMPLE_DOCUMENT_ID, view: { ...active, state: 'unknown', stage: 'done', cancellation, report: detail().report }, busy: null };
    throw Error('Unsupported');
  });
  render(panel());
  await userEvent.click(await screen.findByText('Show saved details'));
  await waitFor(() => expect(screen.getAllByText('Cancellation requested. Waiting for the cluster.')).toHaveLength(2));
  const current = screen.getByRole('region', { name: 'Send and cluster tests' });
  await userEvent.click(within(current).getByRole('button', { name: change === 'outcome' ? 'Retry cancellation' : 'Check cluster status' }));
  if (change === 'outcome') {
    await waitFor(() => expect(screen.getAllByText('Cannot cancel with this cluster identity')).toHaveLength(2));
    expect(screen.queryByText('Cancellation requested. Waiting for the cluster.')).toBeNull();
  } else {
    await waitFor(() => expect(screen.getAllByText('newly-finished')).toHaveLength(2));
    expect(screen.getAllByText('Cancellation requested. Waiting for the cluster.')).toHaveLength(2);
  }
});

test('first cancellation after restart announces progress while the native control operation owns the wait', async () => {
  vi.stubGlobal('isTauri', true);
  let finish: (value: unknown) => void = () => {};
  const detail = { id, documentId: SAMPLE_DOCUMENT_ID, model: 'model', capturedAt: 1, report: null, interrupted: true, canCheck: true, canRemove: false, canCancel: true, cancellation: null, detailState: 'saved', availability: null };
  mockIPC(command => {
    if (command === 'send_history') return [{ id, source, targetDisplay: 'team-dev', targetUids: { serviceNamespace: 'control', serviceUid: 's', deploymentNamespace: 'control', deploymentUid: 'd', watchedNamespace: 'agents' }, provider: 'team-model', plannedAgentName: 'agent', agentUid: 'uid', verdict: 'unknown', time: 1 }];
    if (command === 'send_history_details') return detail;
    if (command === 'send_history_cancel') return new Promise(resolve => { finish = resolve; });
    throw Error('Unsupported');
  });
  render(<SendHistory revision="same" />);
  await userEvent.click(await screen.findByText('Show saved details'));
  await userEvent.click(await screen.findByRole('button', { name: 'Cancel tests' }));
  expect(await screen.findByText('Cancelling tests…')).toBeTruthy();
  finish({ ...detail, cancellation: 'requested' });
  expect(await screen.findByText('Cancellation requested. Waiting for the cluster.')).toBeTruthy();
});

test('unknown cancellation vocabulary is rejected at both IPC boundaries', async () => {
  native({ ...active, cancellation: 'stopped-remotely' });
  await expect(sendCall('send_status')).rejects.toThrow('Send information could not be read');
  mockIPC(() => ({ id, documentId: null, model: null, capturedAt: null, report: null, interrupted: true, canCheck: true, canRemove: false, canCancel: true, cancellation: 'stopped-remotely', detailState: 'saved', availability: null }));
  await expect(sendHistoryDetail('send_history_details', id)).rejects.toThrow('Saved details could not be read');
});

// The native/report parser must accept a real cancelled outcome, not only its label.
test('cancelled report crosses the strict Send boundary', async () => {
  native({ ...active, state: 'complete', stage: 'done', report, cancellation: 'confirmed' });
  const value = await sendCall('send_status');
  expect((value.view as SendView).report?.verdict).toBe('cancelled');
});
