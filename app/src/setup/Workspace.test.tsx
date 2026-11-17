import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createRef } from 'react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../App';
import { setupView } from './progress';
import Setup, { type LocalClusterControl } from './Setup';
import type { SetupStatus } from './state';
import { openLocalSetup } from './test-navigation';

let status: SetupStatus;
let holdStop: (() => void) | undefined;
let holdStart: (() => void) | undefined;
let unreadable = false;
let agentName = 'my-agent';
let agentsReadable = true;
const ipc = vi.fn((command: string) => {
  if (command === 'setup_status') { if (unreadable) throw Error('unreadable'); return status; }
  if (command === 'setup_requirements') return null;
  if (command === 'setup_cancel_local') {
    return new Promise(resolve => { holdStop = () => { status = { selected: 'localVm', localVm: 'cancelled', model: 'locked' }; resolve(status); }; });
  }
  if (command === 'setup_select_local') {
    return new Promise(resolve => { holdStart = () => { status = { selected: 'localVm', localVm: 'starting', model: 'locked', workspaceProgress: { phase: 'starting', elapsedMs: 18000, bytes: null, totalBytes: null, remainingMs: null } }; resolve(status); }; });
  }
  if (command === 'agents_list') { if (!agentsReadable) throw Error('unreadable'); return [{ id: '00000000-0000-4000-8000-000000000001', name: agentName, path: '/agents/my-agent.agent.md', available: true }]; }
  if (command === 'agent_creation_root') return '/agents';
  if (command === 'cluster_contexts') return [];
  throw Error('Unexpected command: ' + command);
});
beforeEach(() => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  status = { selected: 'localVm', localVm: 'ready', model: 'available' };
  agentName = 'my-agent'; agentsReadable = true;
  unreadable = false; holdStop = undefined; holdStart = undefined; ipc.mockClear(); mockIPC(ipc);
});
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

async function requestStop() {
  const user = userEvent.setup(); render(<App />); await openLocalSetup(user);
  await user.click(await screen.findByRole('button', { name: 'Stop the workspace' }));
  expect(ipc.mock.calls.map(([command]) => command)).not.toContain('setup_cancel_local');
  await user.click(await screen.findByRole('button', { name: 'Stop workspace' }));
  return user;
}
async function stop() {
  const user = await requestStop();
  await act(async () => holdStop?.());
  await screen.findByRole('button', { name: 'Start' });
  return user;
}

test.each(['starting', 'ready', 'failed'] as const)('the local place keeps its title while %s', async localVm => {
  status = { selected: 'localVm', localVm, model: localVm === 'ready' ? 'available' : 'locked' };
  render(<App />); await openLocalSetup();
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('This computer');
});

test('Stop keeps This computer and native selection instead of first-run setup', async () => {
  await stop();
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('This computer');
  expect(document.querySelector('.host-chip')?.textContent).toContain('This computer');
  expect(document.querySelector('.host-chip')?.textContent).not.toContain('No cluster selected');
  expect(screen.queryByRole('button', { name: 'Continue' })).toBeNull();
  expect(screen.queryByRole('radio')).toBeNull();
  expect(screen.queryByRole('list', { name: 'Setup steps' })).toBeNull();
  expect(screen.getByText('my-agent')).toBeTruthy();
  expect(screen.getByText('/agents')).toBeTruthy();
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_cancel_local')).toHaveLength(1);
  expect(ipc.mock.calls.map(([command]) => command)).not.toContain('setup_change_local');
});

test('the active Clusters tab opens Places without losing the stopped workspace or restarting it', async () => {
  const user = await stop();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  const row = await screen.findByRole('region', { name: 'This computer' });
  await user.click(within(row).getByRole('button', { name: 'Start' }));
  expect(within(screen.getByRole('region', { name: 'Workspace' })).getByText('Stopped', { selector: '.pill' })).toBeTruthy();
  expect(screen.queryByRole('list', { name: 'Setup steps' })).toBeNull();
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_cancel_local')).toHaveLength(1);
  expect(ipc.mock.calls.filter(([command]) => ['setup_select_local', 'setup_change_local'].includes(command))).toHaveLength(0);
});

test('Stopping overrides the last ready snapshot and disables Start until acknowledged', async () => {
  await requestStop();
  const card = screen.getByRole('region', { name: 'Workspace' });
  expect(within(card).getByText('Stopping', { selector: '.pill' })).toBeTruthy();
  expect((within(card).getByRole('button', { name: 'Start' }) as HTMLButtonElement).disabled).toBe(true);
  expect(document.querySelector('.status-bar')?.textContent).toContain('Workspace stopping');
  expect(within(card).queryByText('Running', { selector: '.pill' })).toBeNull();
});

test('restart shows observed elapsed time without a bar and Cancel stays on This computer', async () => {
  const user = await stop();
  await user.click(screen.getByRole('button', { name: 'Start' }));
  await act(async () => holdStart?.());
  expect(await screen.findByText(/0:18/)).toBeTruthy();
  expect(screen.queryByRole('progressbar')).toBeNull();
  expect(document.querySelector('.status-bar')?.textContent).not.toContain('step');
  expect(localStorage.getItem('bridge.firstTest.v1')).not.toBe('pending');
  await user.click(screen.getByRole('button', { name: 'Cancel' }));
  await act(async () => holdStop?.());
  expect(await screen.findByRole('button', { name: 'Start' })).toBeTruthy();
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('This computer');
});

test('returning mode survives navigation and start failure offers typed recovery on the place', async () => {
  const user = await stop();
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('This computer');
  await user.click(screen.getByRole('button', { name: 'Start' }));
  await act(async () => holdStart?.());
  status = { selected: 'localVm', localVm: 'failed', model: 'locked', localVmError: 'retained diagnostic', workspaceFailure: { cause: 'services', operation: 'start', codes: [] } };
  await act(async () => window.dispatchEvent(new Event('focus')));
  expect(await screen.findByRole('button', { name: 'Try again' })).toBeTruthy();
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('This computer');
  await user.click(screen.getByRole('button', { name: 'Show details' }));
  expect(screen.getByText('retained diagnostic')).toBeTruthy();
});

test('Designer emergency Stop uses the same preserving stop without another confirmation', async () => {
  const ref = createRef<LocalClusterControl>();
  render(<Setup ref={ref} onOpenExample={() => {}} />);
  await openLocalSetup();
  await act(async () => ref.current?.stop());
  expect(ipc.mock.calls.map(([command]) => command)).toContain('setup_cancel_local');
  expect(ipc.mock.calls.map(([command]) => command)).not.toContain('setup_change_local');
  await act(async () => holdStop?.());
  expect(await screen.findByRole('button', { name: 'Start' })).toBeTruthy();
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('This computer');
});

test.each(['closed', 'failed'] as const)('terminal %s after Stop never offers an inert restart', async phase => {
  await stop();
  status = { selected: 'localVm', localVm: phase, model: 'locked', ...(phase === 'failed' ? { workspaceFailure: { cause: 'stopUnconfirmed' as const, operation: 'stop' as const, codes: [] } } : {}) };
  await act(async () => window.dispatchEvent(new Event('focus')));
  await screen.findAllByText(/Quit Bridge and reopen it/, { selector: 'p' });
  expect(screen.queryByRole('button', { name: 'Start' })).toBeNull();
  const retry = screen.queryByRole('button', { name: 'Try again' });
  if (retry) expect((retry as HTMLButtonElement).disabled).toBe(true);
});

test('restart reaching ready keeps the place heading and requires an explicit AI connection', async () => {
  const user = await stop();
  await user.click(screen.getByRole('button', { name: 'Start' }));
  await act(async () => holdStart?.());
  status = { selected: 'localVm', localVm: 'ready', model: 'available' };
  await act(async () => window.dispatchEvent(new Event('focus')));
  expect(await screen.findByRole('button', { name: 'Stop' })).toBeTruthy();
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('This computer');
  expect(screen.getByRole('button', { name: 'Connect the AI' })).toBeTruthy();
  expect(ipc.mock.calls.map(([command]) => command)).not.toContain('setup_connect_model');
  expect(localStorage.getItem('bridge.firstTest.v1')).not.toBe('pending');
});

test('Keep running returns keyboard focus to the Stop trigger', async () => {
  const user = userEvent.setup(); render(<App />); await openLocalSetup(user);
  const trigger = await screen.findByRole('button', { name: 'Stop the workspace' });
  await user.click(trigger);
  await user.click(screen.getByRole('button', { name: 'Keep running' }));
  expect(document.activeElement).toBe(trigger);
  expect(ipc.mock.calls.map(([command]) => command)).not.toContain('setup_cancel_local');
});

test.each([true, false])('returning from Designer refreshes saved agents, initially readable=%s', async initiallyReadable => {
  agentsReadable = initiallyReadable;
  const user = await stop();
  await screen.findByText(initiallyReadable ? 'my-agent' : 'Agent list unavailable');
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  agentName = 'new-agent'; agentsReadable = true;
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(await screen.findByText('new-agent')).toBeTruthy();
  expect(within(screen.getByRole('region', { name: 'Workspace' })).queryByText('my-agent')).toBeNull();
});

test('terminal closed is Manage, not a Start promise on the Places row', () => {
  expect(setupView({ selected: 'localVm', localVm: 'closed', model: 'locked' }, { workspace: true }).action).toBe('Manage');
  expect(setupView({ selected: 'localVm', localVm: 'cancelled', model: 'locked' }, { workspace: true }).action).toBe('Start');
});

test('unreadable stopped status cannot authorize a restart', async () => {
  await stop(); unreadable = true;
  await act(async () => window.dispatchEvent(new Event('focus')));
  await screen.findByText('The setup choice could not be read or saved.');
  expect((screen.getByRole('button', { name: 'Start' }) as HTMLButtonElement).disabled).toBe(true);
});
