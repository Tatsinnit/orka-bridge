import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Run from './Run';
import { SAMPLE_DOCUMENT_ID as documentId } from './agent-api';
import type { RunSnapshot } from './run-api';
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });
const running: RunSnapshot = { documentId, busy: { documentId, name: 'reader' }, state: 'running', canRun: false, canCancel: true, cancellation: 'none', stage: 'observe', report: null, acknowledged: [], pending: null, error: null };
const cancelling: RunSnapshot = { ...running, canCancel: false, cancellation: 'cancelling', stage: 'cancel' };
const passed = { id: 'finished', question: 'What did it say?', phase: 'Succeeded', answer: 'Kept answer', completedTools: ['web_fetch'], assertions: [{ id: 'a', kind: 'contains' as const, expected: 'Kept', verdict: 'passed' as const }], verdict: 'passed' as const };
const cancelled = { ...passed, id: 'cancelled', phase: 'Cancelled', answer: null, assertions: [], verdict: 'cancelled' as const };
const done: RunSnapshot = { ...running, busy: null, state: 'complete', stage: 'done', canRun: true, canCancel: false, cancellation: 'cancelled', report: { verdict: 'cancelled', cases: [cancelled, passed] } };
const unconfirmed: RunSnapshot = { ...done, cancellation: 'unconfirmed', canRetryCancellation: true, canCheckCancellation: true, report: { verdict: 'unknown', cases: [passed] } };
function native(value: RunSnapshot = running) { vi.stubGlobal('isTauri', true); mockIPC(() => value); }
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }

test('Cancel is explicit, document-bound, single-flight and becomes a status, not a disabled button', async () => {
  vi.stubGlobal('isTauri', true); const reply = deferred<RunSnapshot>(); let value = running;
  const ipc = vi.fn(command => command === 'agent_run_cancel' ? reply.promise : value); mockIPC(ipc);
  render(<Run blocked stale />); // Unsaved edits do not block stopping already-started work.
  const cancel = await screen.findByRole('button', { name: 'Cancel tests' });
  expect(cancel.className).toContain('button-secondary'); expect(cancel.closest('.long-card-head')).toBeTruthy();
  await userEvent.dblClick(cancel);
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'agent_run_cancel')).toEqual([['agent_run_cancel', { documentId }]]);
  expect(screen.queryByRole('button', { name: 'Cancel tests' })).toBeNull();
  expect(await screen.findByText(/Cancelling tests · waiting for the test worker to stop/)).toBeTruthy();
  value = cancelling; await act(async () => reply.resolve(value));
  expect(screen.queryByRole('button', { name: 'Run tests' })).toBeNull();
});
test.each([false, undefined])('older image canCancel=%s never offers cancellation', async canCancel => {
  const value = { ...running, canCancel }; if (canCancel === undefined) delete value.canCancel;
  native(value); render(<Run />);
  await screen.findByText('Waiting for answers and evaluating tests');
  expect(screen.queryByRole('button', { name: 'Cancel tests' })).toBeNull();
});
test.each(['agent_run_cancel', 'agent_run_check_cancellation'] as const)('unconfirmed recovery %s is explicit and returns to Cancelling', async command => {
  vi.stubGlobal('isTauri', true); let value = unconfirmed;
  const ipc = vi.fn(cmd => { if (cmd === command) value = cancelling; return value; }); mockIPC(ipc);
  const { unmount } = render(<Run />); await screen.findByText('Cancellation not confirmed. Work may still be running on this computer.', { selector: 'p:not(.sr-only)' });
  expect(screen.getByText('Not confirmed', { selector: '.pill' }).className).toContain('status-needs-you');
  expect(ipc.mock.calls.every(([cmd]) => cmd === 'agent_run_status')).toBe(true);
  await userEvent.click(screen.getByRole('button', { name: command === 'agent_run_cancel' ? 'Retry cancellation' : 'Check again' }));
  expect(ipc.mock.calls.filter(([cmd]) => cmd === command)).toEqual([[command, { documentId }]]);
  expect(await screen.findByText('Cancelling', { selector: '.pill' })).toBeTruthy();
  unmount(); render(<Run />); await screen.findByText('Cancelling', { selector: '.pill' });
  expect(ipc.mock.calls.filter(([cmd]) => cmd === command)).toHaveLength(1);
});
test('unconfirmed overrides error and unknown verdict even without recovery authority', async () => {
  native({ ...unconfirmed, error: 'session', canRetryCancellation: false, canCheckCancellation: false }); render(<Run />);
  await screen.findByText('Not confirmed', { selector: '.pill' });
  expect(screen.queryByRole('button', { name: 'Retry cancellation' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Check again' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Stop the local cluster' })).toBeTruthy();
  expect(screen.getByRole('alert').textContent).toContain('not available');
});
test('Check alone follows its native permission, without inferring Retry from case phases', async () => {
  native({ ...unconfirmed, canRetryCancellation: false }); render(<Run />);
  expect(await screen.findByRole('button', { name: 'Check again' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Retry cancellation' })).toBeNull();
});
test('confirmed cancellation keeps finished evidence before neutral Cancelled rows and shows counts', async () => {
  native(done); render(<Run />);
  expect(await screen.findByText('Tests cancelled. The test worker on this computer has stopped. 1 passed, 0 failed, 1 cancelled.', { selector: 'p:not(.sr-only)' })).toBeTruthy();
  expect(screen.getByText('Cancelled', { selector: '.pill' }).className).toContain('status-not-started');
  const rows = [...document.querySelectorAll('.test-result')];
  expect([...document.querySelectorAll('.test-result > summary code')].map(node => node.textContent)).toEqual(['finished', 'cancelled']);
  await userEvent.click(rows[0].querySelector('summary')!);
  expect(screen.getByText('Kept answer')).toBeTruthy();
  expect(within(rows[1] as HTMLElement).getByText(/Actions already taken are not undone/)).toBeTruthy();
  expect(rows[1].querySelector('.status-not-started')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Run tests' }).hasAttribute('disabled')).toBe(false);
});
test.each([done, unconfirmed, { ...done, cancellation: 'beforeStart' as const, report: { verdict: 'cancelled' as const, cases: [] } }])('cancellation $cancellation is announced even when the edited files make results stale', async snapshot => {
  native(snapshot); render(<Run stale />);
  await screen.findByText('Not run since your edit');
  await waitFor(() => expect(document.querySelector('.test-announcement')?.textContent).toMatch(/Tests cancelled\.|Cancellation not confirmed\.|Cancelled before any test started\./));
});
test('tests that finished before cancellation retain a real failure and are never called a pass', async () => {
  native({ ...done, report: { verdict: 'cancelled', cases: [{ ...passed, verdict: 'failed', assertions: [{ ...passed.assertions[0], verdict: 'failed' }] }] } }); render(<Run />);
  expect(await screen.findByText('Tests finished before cancellation. The test worker on this computer has stopped. 0 passed, 1 failed, 0 cancelled.', { selector: 'p:not(.sr-only)' })).toBeTruthy();
  expect(screen.getByText('Failed', { selector: '.test-result span' })).toBeTruthy();
});
test('confirmed cancellation counts retained unknown outcomes as well as passed ones', async () => {
  native({ ...done, report: { verdict: 'cancelled', cases: [passed, { ...passed, id: 'unobserved', answer: null, verdict: 'unknown', assertions: [{ ...passed.assertions[0], verdict: 'unknown' }] }] } });
  render(<Run />);
  expect(await screen.findByText('Tests finished before cancellation. The test worker on this computer has stopped. 1 passed, 0 failed, 0 cancelled, 1 unknown.', { selector: 'p:not(.sr-only)' })).toBeTruthy();
  expect(document.querySelector('.test-announcement')?.textContent).toContain('1 unknown.');
});
test('pre-start cancellation has its own announcement and no invented result rows', async () => {
  native({ ...done, cancellation: 'beforeStart', report: { verdict: 'cancelled', cases: [] } }); render(<Run />);
  await screen.findByText('Cancelled before any test started.', { selector: 'p:not(.sr-only)' });
  expect(document.querySelector('.test-announcement')?.textContent).toBe('Cancelled before any test started.');
  expect(document.querySelector('.test-result')).toBeNull();
});
test('unconfirmed recovery can use the native-admitted Run after a restart without sticky Stop state', async () => {
  native({ ...unconfirmed, error: 'session', canRetryCancellation: false, canCheckCancellation: false });
  render(<Run />);
  const run = await screen.findByRole('button', { name: 'Run tests' });
  await waitFor(() => expect(run.hasAttribute('disabled')).toBe(false));
});
test('status slower than the polling interval still loads and reaches cancellation recovery', async () => {
  vi.useFakeTimers(); vi.stubGlobal('isTauri', true); let value = running; let reads = 0;
  mockIPC(command => {
    if (command === 'agent_run_status') { reads++; return new Promise(resolve => setTimeout(() => resolve(value), 600)); }
    value = cancelling; return value;
  });
  await act(async () => { render(<Run />); });
  await act(async () => { await vi.advanceTimersByTimeAsync(620); });
  expect(screen.queryByRole('button', { name: 'Cancel tests' })).not.toBeNull();
  expect(reads).toBe(1);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel tests' }));
  await act(async () => { await vi.advanceTimersByTimeAsync(10); });
  value = unconfirmed;
  await act(async () => { await vi.advanceTimersByTimeAsync(1600); });
  expect(screen.queryByRole('button', { name: 'Retry cancellation' })).not.toBeNull();
});
test('an old poll cannot restore Cancel after its action response', async () => {
  vi.stubGlobal('isTauri', true); const poll = deferred<RunSnapshot>(); let reads = 0;
  mockIPC(command => command === 'agent_run_cancel' ? cancelling : ++reads === 1 ? running : poll.promise);
  render(<Run />); const cancel = await screen.findByRole('button', { name: 'Cancel tests' });
  await waitFor(() => expect(reads).toBeGreaterThan(1));
  await userEvent.click(cancel); await screen.findByText('Cancelling', { selector: '.pill' });
  await act(async () => poll.resolve(running));
  expect(screen.queryByRole('button', { name: 'Cancel tests' })).toBeNull();
});
test('a previous document action reply cannot replace the current document', async () => {
  vi.stubGlobal('isTauri', true); const reply = deferred<RunSnapshot>();
  const other = '00000000-0000-4000-8000-000000000001';
  mockIPC((command, args) => command === 'agent_run_cancel' ? reply.promise : args && 'documentId' in args && args.documentId === documentId ? running : { ...running, documentId: other, busy: null, state: 'idle', stage: null, canRun: true, canCancel: false });
  const view = render(<Run />); await userEvent.click(await screen.findByRole('button', { name: 'Cancel tests' }));
  view.rerender(<Run documentId={other} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests' }).hasAttribute('disabled')).toBe(false));
  await act(async () => reply.resolve(cancelling));
  expect(screen.queryByText('Cancelling', { selector: '.pill' })).toBeNull();
});
