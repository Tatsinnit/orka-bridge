import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Run from './Run';
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
const idle = { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'idle', canRun: true, report: null, acknowledged: [], pending: null, error: null };
const complete = { ...idle, state: 'complete', report: { verdict: 'unknown', cases: [{ id: 'published-checksum', question: 'What did the page say?', phase: 'Succeeded', answer: '<script>not executable</script>', completedTools: [], assertions: [{ id: 'checksum-from-page', kind: 'contains', expected: 'checksum', verdict: 'passed' }, { id: 'fetched-page', kind: 'toolCalled', expected: 'web_fetch', verdict: 'unknown' }], verdict: 'unknown' }] } };
test('Run is explicit, duplicate clicks and remounts only observe existing work', async () => {
  vi.stubGlobal('isTauri', true); let value: unknown = idle;
  const ipc = vi.fn(command => {
    if (command === 'agent_run') value = { ...idle, state: 'running', canRun: false };
    return value;
  }); mockIPC(ipc);
  const view = render(<Run />); const button = await screen.findByRole('button', { name: 'Run tests' });
  await userEvent.dblClick(button); expect(ipc.mock.calls.filter(([cmd]) => cmd === 'agent_run')).toHaveLength(1);
  view.unmount(); render(<Run />); expect(await screen.findByText('Starting your tests')).toBeTruthy();
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'agent_run')).toHaveLength(1);
  const live = screen.getAllByRole('status').find(node => node.classList.contains('sr-only'));
  expect(live).toBeTruthy();
  value = complete;
  expect(await screen.findByText('<script>not executable</script>')).toBeTruthy();
  expect(screen.getByText('published-checksum').closest('details')?.open).toBe(true);
  expect(screen.getByText('What did the page say?')).toBeTruthy();
  expect(document.querySelector('script')).toBeNull();
  expect(screen.getByText('A call to web_fetch was not observed; this result is unknown.')).toBeTruthy();
  expect(live?.isConnected).toBe(true);
  expect(live?.textContent).toBe('Test result unknown.');
});
test.each(['passed', 'failed'])('unknown local operation overrides a retained %s report announcement', async verdict => {
  vi.stubGlobal('isTauri', true);
  mockIPC(() => ({ ...complete, state: 'unknown', report: { verdict, cases: [{ id: 'a', question: 'What did the page say?', phase: 'Succeeded', answer: verdict === 'passed' ? 'checksum' : 'other', completedTools: [], verdict, assertions: [{ id: 'text', kind: 'contains', expected: 'checksum', verdict }] }] } }));
  render(<Run />);
  await waitFor(() => expect(screen.getAllByRole('status').find(node => node.classList.contains('test-announcement'))?.textContent).toBe('Test result unknown.'));
});
test('model connection advice follows the acknowledged model state, not Run availability', async () => {
  vi.stubGlobal('isTauri', true);
  let modelConnected = false;
  mockIPC(() => ({ ...idle, state: 'running', canRun: false, modelConnected }));
  const view = render(<Run />);
  expect(screen.queryByText(/Connect the AI in Clusters first\./)).toBeNull();
  expect(await screen.findByText(/Connect the AI in Clusters first\./)).toBeTruthy();
  modelConnected = true;
  view.unmount();
  render(<Run />);
  expect(await screen.findByText('Starting your tests')).toBeTruthy();
  expect(screen.queryByText(/Connect the AI in Clusters first\./)).toBeNull();
  expect(screen.queryByRole('button', { name: 'Run tests' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Stop' })).toBeNull();
});
test.each([
  { error: 'model', modelConnected: false, alert: true },
  { error: 'model', modelConnected: true, alert: false },
  { error: 'submit', modelConnected: true, alert: true },
  { error: 'session', modelConnected: true, alert: true },
])('retained $error advice follows native connection state without hiding other errors', async ({ error, modelConnected, alert }) => {
  vi.stubGlobal('isTauri', true);
  mockIPC(() => ({ ...idle, state: 'unknown', error, modelConnected }));
  render(<Run />);
  // Wait for the native response, not the initial disabled rendering.
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests' }).hasAttribute('disabled')).toBe(false));
  expect(!!screen.queryByRole('alert')).toBe(alert);
});
test('uncertain submission names retained receipts and never retries automatically', async () => {
  vi.stubGlobal('isTauri', true); const ipc = vi.fn((command: string) => { expect(command).toBe('agent_run_status'); return { ...idle, state: 'unknown', error: 'submit', acknowledged: [{ kind: 'Agent', name: 'bridge-run-example', uid: 'uid-a' }], pending: { kind: 'Task', name: 'bridge-task-example' } }; }); mockIPC(ipc); render(<Run />);
  expect(await screen.findAllByText('brid…mple')).toHaveLength(2);
  expect(screen.getByRole('button', { name: 'Copy Possibly created Task', hidden: true })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Copy Agent', hidden: true })).toBeTruthy();
  expect(ipc.mock.calls.every(([cmd]) => cmd === 'agent_run_status')).toBe(true);
});
test.each([null, { ...idle, modelConnected: 'true' }, { ...idle, state: ['idle'] }, { ...idle, error: 'PRIVATE_DIAGNOSTIC' }, { ...complete, extra: 'PRIVATE_DIAGNOSTIC' }, { ...complete, report: { ...complete.report, verdict: 'passed', cases: [{ secret: 'PRIVATE_DIAGNOSTIC' }] } }])('malformed Run data never renders raw diagnostics', async value => {
  vi.stubGlobal('isTauri', true); mockIPC(() => value); render(<Run />); expect(await screen.findByText('Run information is not supported by this build.')).toBeTruthy(); expect(document.body.textContent).not.toContain('PRIVATE_DIAGNOSTIC');
});
test.each(['submit', 'observe', null])('uncertain %s is not labelled failed or not started', async error => {
  vi.stubGlobal('isTauri', true);
  mockIPC(() => ({ ...idle, state: 'unknown', error }));
  const { container } = render(<Run />);
  expect((await screen.findByText('Result unknown')).closest('.pill')).toBeNull();
  expect(container.querySelector('.pill.status-stale')?.textContent).toBe('Stale');
  expect(container.querySelector('.pill.status-failed')).toBeNull();
});
test('an older pass is a stale pill and never a green result', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC(() => ({ ...complete, report: { ...complete.report, verdict: 'passed' } }));
  const { container } = render(<Run stale />);
  await screen.findByText('Not run since your edit');
  expect(container.querySelector('.pill.status-stale')).toBeTruthy();
  expect(container.querySelector('.pill.status-passed')).toBeNull();
  expect(screen.getByText('<script>not executable</script>').tagName).toBe('P');
});
test.each([
  ['capture', 'Reading your saved files'], ['model', 'Checking the local model'],
  ['submit', 'Starting your tests'], ['observe', 'Waiting for answers and evaluating tests'],
])('native %s stage is factual, with no local Stop or disabled primary', async (stage, text) => {
  vi.stubGlobal('isTauri', true);
  mockIPC(() => ({ ...idle, state: 'running', canRun: false, stage, model: 'captured-model' }));
  render(<Run />);
  expect(await screen.findByText(text)).toBeTruthy();
  expect(screen.getByText('captured-model')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Run tests' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Stop' })).toBeNull();
});
test('preview never fabricates a native test or sends Run IPC', async () => {
  vi.stubGlobal('isTauri', false); const ipc = vi.fn(); mockIPC(ipc); render(<Run />);
  expect(await screen.findByText('Run is available only in the native app.')).toBeTruthy(); expect(ipc).not.toHaveBeenCalled();
});
