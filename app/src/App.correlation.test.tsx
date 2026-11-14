import { openLocalSetup } from './setup/test-navigation';
import { useEffect } from 'react';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import type { RunSnapshot } from './designer/run-api';
import App from './App';

const result = vi.hoisted(() => ({ snapshot: undefined as RunSnapshot | undefined, dirty: false }));
// Inject the Designer callback boundary; exercise App, Setup and Steps together.
vi.mock('./designer/Example', () => ({ default: function Example({ onRun, onSaveState }: { onRun: (value: RunSnapshot | undefined, documentId: string) => void; onSaveState?: (value: 'dirty', documentId: string) => void }) {
  useEffect(() => {
    if (result.dirty) onSaveState?.('dirty', '9b2b93c4-375c-4815-9717-39852a96e411');
    onRun(result.snapshot, result.snapshot?.documentId ?? '9b2b93c4-375c-4815-9717-39852a96e411');
  }, [onRun, onSaveState]);
  return <h1>Agent</h1>;
} }));
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); result.dirty = false; });

test('Check again replaces an in-flight sample observation without accepting its late reply or running', async () => {
  result.snapshot = undefined;
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  let old!: (value: unknown) => void; let reads = 0;
  const unknown = { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'unknown', canRun: false, report: null, acknowledged: [], pending: null, error: null };
  const ipc = vi.fn((command: string) => {
    if (command === 'agent_run_status') return ++reads === 1 ? new Promise(resolve => { old = resolve; }) : unknown;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'setup_example') return { agent: 'Read.', tests: 'Checks' };
    throw Error('unavailable');
  });
  mockIPC(ipc); const user = userEvent.setup(); render(<App />); await openLocalSetup(user);
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await user.click(screen.getByRole('button', { name: 'Read its instructions' }));
  await waitFor(() => expect(reads).toBe(1));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Check again' }));
  await waitFor(() => expect(reads).toBe(2));
  old({ ...unknown, state: 'running' });
  await new Promise(resolve => setTimeout(resolve, 50));
  expect(screen.getByRole('button', { name: 'Check again' })).toBeTruthy();
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')?.textContent).toContain('Result unknown');
  expect(ipc.mock.calls.some(([command]) => command === 'agent_run')).toBe(false);
});

test('a completed sample retires held setup intent even when the next status poll is unreadable', async () => {
  const sample = '9b2b93c4-375c-4815-9717-39852a96e411';
  result.snapshot = undefined; result.dirty = true;
  vi.stubGlobal('isTauri', true);
  localStorage.setItem('bridge.welcomeSeen.v1', '1');
  localStorage.setItem('bridge.firstTest.v1', 'pending');
  let unreadable = false;
  const idle = { documentId: sample, state: 'idle', busy: null, canRun: true, report: null, acknowledged: [], pending: null, error: null };
  const ipc = vi.fn((command: string) => {
    if (command === 'agent_run_status') return unreadable ? Promise.reject('unavailable') : result.snapshot ?? idle;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_connect_model') return { ready: true };
    throw Error('unavailable');
  });
  mockIPC(ipc); const user = userEvent.setup(); render(<App />); await openLocalSetup(user);
  await user.click(screen.getByRole('button', { name: 'Read its instructions' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText('Step 5 of 5');
  result.snapshot = { ...idle, state: 'complete', report: { verdict: 'passed', cases: [{ id: 'check', question: 'Read?', phase: 'Succeeded', answer: 'yes', completedTools: [], assertions: [{ id: 'answer', kind: 'contains', expected: 'yes', verdict: 'passed' }], verdict: 'passed' }] } };
  const navigation = within(screen.getByRole('navigation'));
  await navigation.findByText('Local cluster ready');
  expect(screen.queryByText('Step 5 of 5')).toBeNull();
  unreadable = true;
  await navigation.findByText('Test result unknown');
  expect(screen.queryByText('Step 5 of 5')).toBeNull();
  expect(ipc.mock.calls.some(([command]) => command === 'agent_run')).toBe(false);
});

test.each(['unavailable', 'other', 'sample'] as const)('setup keeps %s Run evidence separate from workspace readiness', async kind => {
  const sample = '9b2b93c4-375c-4815-9717-39852a96e411';
  result.snapshot = kind === 'unavailable' ? undefined : {
    documentId: kind === 'sample' ? sample : '00000000-0000-4000-8000-000000000001', busy: null,
    state: 'complete', canRun: true, report: { verdict: 'passed', cases: [{ id: 'check', question: 'Read?', phase: 'Succeeded', answer: 'yes', completedTools: [], assertions: [{ id: 'answer', kind: 'contains', expected: 'yes', verdict: 'passed' }], verdict: 'passed' }] }, acknowledged: [], pending: null, error: null,
  };
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  mockIPC(command => {
    if (command === 'agent_run_status') return result.snapshot;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'setup_example') return { agent: '---\nname: web-reader\n---\nRead.', tests: 'Checks' };
    throw Error('unavailable');
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  if (kind === 'sample') await user.click(screen.getByRole('button', { name: 'Completed setup steps' }));
  await waitFor(() => expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')?.textContent)
    .toContain({ unavailable: 'Result unknown', other: 'Needs you', sample: 'Its test passed' }[kind]));
  const navigation = within(screen.getByRole('navigation'));
  expect(navigation.getByText({ unavailable: 'Test result unknown', other: 'Local cluster ready', sample: 'Local cluster ready' }[kind])).toBeTruthy();
});
