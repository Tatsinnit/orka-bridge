import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../App';
import type { SetupStatus } from './state';
import { openLocalSetup } from './test-navigation';

let status: SetupStatus;
let unreadable = false;
let agentName = 'my-agent';
let quitFails = false;
let helpFails = false;
let helpPending: Promise<void> | null = null;
const ipc = vi.fn((command: string) => {
  if (command === 'setup_status') { if (unreadable) throw Error('unreadable'); return status; }
  if (command === 'setup_requirements') return null;
  if (command === 'setup_select_local') return status = { selected: 'localVm', localVm: 'starting', model: 'locked', workspaceProgress: { phase: 'starting', elapsedMs: 18000, bytes: null, totalBytes: null, remainingMs: null } };
  if (command === 'setup_open_help') { if (helpPending) return helpPending; if (helpFails) throw Error('unavailable'); return; }
  if (command === 'quit_bridge') { if (quitFails) throw Error('unavailable'); return; }
  if (command === 'agents_list') return [{ id: '00000000-0000-4000-8000-000000000001', name: agentName, path: '/agents/my-agent.agent.md', available: true }];
  if (command === 'agent_creation_root') return '/agents';
  if (command === 'cluster_contexts') return [];
  throw Error('Unexpected command: ' + command);
});
beforeEach(() => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  localStorage.setItem('bridge.firstTest.v1', 'attempted');
  status = { selected: 'localVm', localVm: 'ready', model: 'available' };
  agentName = 'my-agent'; unreadable = false; quitFails = false; helpFails = false; helpPending = null; ipc.mockClear(); mockIPC(ipc);
});
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

async function open() { const user = userEvent.setup(); render(<App />); await openLocalSetup(user); return user; }
async function observe(value: SetupStatus) { status = value; await act(async () => window.dispatchEvent(new Event('focus'))); }

test('a returning ready workspace has no lifecycle action and keeps its files beside the AI', async () => {
  const user = await open();
  const card = await screen.findByRole('region', { name: 'Workspace' });
  expect(within(card).getByText('Ready', { selector: '.pill' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: /^(Stop|Start)( the workspace)?$/ })).toBeNull();
  expect(await within(card).findByText('my-agent')).toBeTruthy();
  expect(within(card).getByText('/agents')).toBeTruthy();
  expect(card.closest('.workspace-setup-grid')).toBeTruthy();
  expect(screen.getByRole('heading', { name: 'AI', level: 2 })).toBeTruthy();
  await user.click(within(card).getByRole('button', { name: "What's inside" }));
  expect(within(card).getByText(/Uses about 4 GB of memory while Bridge is open. Quit Bridge to free it./)).toBeTruthy();
  expect(screen.queryByRole('radio', { name: 'On this computer' })).toBeNull();
});

test('reopening shows observed Starting and elapsed time without another Start command', async () => {
  status = { selected: 'localVm', localVm: 'starting', model: 'locked', workspaceProgress: { phase: 'starting', elapsedMs: 18000, bytes: null, totalBytes: null, remainingMs: null } };
  await open();
  const card = await screen.findByRole('region', { name: 'Workspace' });
  expect(within(card).getByText('Starting', { selector: '.pill' })).toBeTruthy();
  expect(within(card).getByText('0:18 so far')).toBeTruthy();
  expect(screen.getByRole('heading', { name: 'AI', level: 2 }).closest('.card')).toBeTruthy();
  expect(screen.queryByRole('button', { name: /^Start$/ })).toBeNull();
  expect(ipc.mock.calls.map(([command]) => command)).not.toContain('setup_select_local');
  await observe({ selected: 'localVm', localVm: 'ready', model: 'available' });
  expect(await within(card).findByText('Ready', { selector: '.pill' })).toBeTruthy();
});

test('the saved workspace awaits native auto-start without flashing a stopped action', async () => {
  status = { selected: 'localVm', localVm: 'idle', model: 'locked' };
  await open();
  const card = await screen.findByRole('region', { name: 'Workspace' });
  expect(within(card).getByText('Starting', { selector: '.pill' })).toBeTruthy();
  expect(within(card).queryByRole('button', { name: /Start|Restart/ })).toBeNull();
  expect(ipc.mock.calls.map(([command]) => command)).not.toContain('setup_select_local');
});

test('an interrupted first setup remains setup after reopening', async () => {
  localStorage.setItem('bridge.firstTest.v1', 'pending');
  await open();
  expect(screen.queryByRole('region', { name: 'Workspace' })).toBeNull();
  expect(screen.getByText('Run your first agent')).toBeTruthy();
});

test.each(['failed', 'cancelled'] as const)('%s workspace restarts on This computer without arming another first test', async localVm => {
  status = { selected: 'localVm', localVm, model: 'locked', ...(localVm === 'failed' ? { localVmError: 'retained diagnostic', workspaceFailure: { cause: 'services' as const, operation: 'start' as const, codes: [] } } : {}) };
  const user = await open();
  await user.click(await screen.findByRole('button', { name: 'Restart workspace' }));
  expect(await screen.findByText('0:18 so far')).toBeTruthy();
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('This computer');
  expect(localStorage.getItem('bridge.firstTest.v1')).not.toBe('pending');
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_select_local')).toHaveLength(1);
});

test('stop failure offers Quit Bridge through native cleanup and keeps a rejected quit retryable', async () => {
  status = { selected: 'localVm', localVm: 'failed', model: 'locked', localVmError: 'private stop diagnostic', workspaceFailure: { cause: 'stopUnconfirmed', operation: 'stop', codes: [] } };
  quitFails = true;
  const user = await open();
  const card = await screen.findByRole('region', { name: 'Workspace' });
  expect(within(card).getByText("The workspace didn't stop.")).toBeTruthy();
  expect(within(card).queryByRole('button', { name: /Restart|Try again|Start/ })).toBeNull();
  const quit = within(card).getByRole('button', { name: 'Quit Bridge' });
  expect(quit.classList.contains('button-primary')).toBe(true);
  await user.click(quit);
  expect(await within(card).findByRole('alert')).toBeTruthy();
  expect((quit as HTMLButtonElement).disabled).toBe(false);
  expect(ipc.mock.calls.filter(([command]) => command === 'quit_bridge')).toHaveLength(1);
  expect(ipc.mock.calls.map(([command]) => command)).not.toContain('setup_select_local');
  await user.click(within(card).getByRole('button', { name: 'Show details' }));
  expect(within(card).getByText('private stop diagnostic')).toBeTruthy();
});

test('returning preflight failure keeps its specific remedy and help action', async () => {
  status = { selected: 'localVm', localVm: 'failed', model: 'locked', preflightFailure: 'hypervisorUnavailable' };
  const user = await open();
  const card = await screen.findByRole('region', { name: 'Workspace' });
  expect(within(card).getByText("Windows Hypervisor Platform isn't available.")).toBeTruthy();
  expect(within(card).getByText('Enable Windows Hypervisor Platform in Windows features, restart, then try again.')).toBeTruthy();
  await user.click(within(card).getByRole('button', { name: 'Show me how' }));
  expect(ipc).toHaveBeenCalledWith('setup_open_help', { topic: 'hypervisor' });
});

test.each(['workspaceUpdateRequired', 'workspaceVersionUnsupported'] as const)('blocked %s restart explains the remedy without hover', async cause => {
  status = { selected: 'localVm', localVm: 'failed', model: 'locked', workspaceFailure: { cause, operation: 'verify', codes: [] } };
  await open();
  const card = await screen.findByRole('region', { name: 'Workspace' });
  expect(within(card).getByText('Open the matching Bridge version.', { selector: 'p' })).toBeTruthy();
  expect((within(card).getByRole('button', { name: 'Restart workspace' }) as HTMLButtonElement).disabled).toBe(true);
});

test('unavailable integration keeps its explanation instead of promising startup', async () => {
  status = { selected: 'localVm', localVm: 'notIntegrated', model: 'locked' };
  await open();
  expect(screen.queryByRole('region', { name: 'Workspace' })).toBeNull();
  expect(screen.getByText('Workspace setup is not included in this build. Nothing will be downloaded or started.')).toBeTruthy();
});

test('returning failure guidance names the offered restart and details controls', async () => {
  status = { selected: 'localVm', localVm: 'failed', model: 'locked', workspaceFailure: { cause: 'services', operation: 'services', codes: [] } };
  await open();
  expect(screen.getByText('Press Restart workspace to restart the workspace. If it happens again, open Show details for help.')).toBeTruthy();
});

test('a rejected help action does not leave a failure notice on a recovered workspace', async () => {
  status = { selected: 'localVm', localVm: 'failed', model: 'locked', preflightFailure: 'hypervisorUnavailable' }; helpFails = true;
  const user = await open();
  await user.click(screen.getByRole('button', { name: 'Show me how' }));
  expect(await screen.findByText("Couldn't open help. Open Show details to read the error.")).toBeTruthy();
  await observe({ selected: 'localVm', localVm: 'ready', model: 'available' });
  await within(screen.getByRole('region', { name: 'Workspace' })).findByText('Ready', { selector: '.pill' });
  expect(screen.queryByText("Couldn't open help. Open Show details to read the error.")).toBeNull();
  await observe({ selected: 'localVm', localVm: 'failed', model: 'locked', workspaceFailure: { cause: 'permission', operation: 'start', codes: [] } });
  await within(screen.getByRole('region', { name: 'Workspace' })).findByText('Bridge does not have permission to start the workspace.');
  expect(screen.queryByText("Couldn't open help. Open Show details to read the error.")).toBeNull();
});

test('a late help rejection cannot enter a newer failure episode', async () => {
  let rejectHelp!: (reason: Error) => void;
  helpPending = new Promise<void>((_, reject) => { rejectHelp = reject; });
  status = { selected: 'localVm', localVm: 'failed', model: 'locked', preflightFailure: 'hypervisorUnavailable' };
  const user = await open();
  await user.click(screen.getByRole('button', { name: 'Show me how' }));
  await observe({ selected: 'localVm', localVm: 'ready', model: 'available' });
  await within(screen.getByRole('region', { name: 'Workspace' })).findByText('Ready', { selector: '.pill' });
  await observe({ selected: 'localVm', localVm: 'failed', model: 'locked', workspaceFailure: { cause: 'permission', operation: 'start', codes: [] } });
  await within(screen.getByRole('region', { name: 'Workspace' })).findByText('Bridge does not have permission to start the workspace.');
  await act(async () => { rejectHelp(Error('old help unavailable')); });
  expect(screen.queryByText("Couldn't open help. Open Show details to read the error.")).toBeNull();
});

test('returning to This computer refreshes saved agents', async () => {
  const user = await open(); await screen.findByText('my-agent');
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  agentName = 'new-agent';
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(await screen.findByText('new-agent')).toBeTruthy();
});

test('unreadable workspace status cannot authorize a restart', async () => {
  status = { selected: 'localVm', localVm: 'failed', model: 'locked', workspaceFailure: { cause: 'services', operation: 'start', codes: [] } };
  await open(); unreadable = true;
  await act(async () => window.dispatchEvent(new Event('focus')));
  await screen.findByText('The setup choice could not be read or saved.');
  expect((screen.getByRole('button', { name: 'Restart workspace' }) as HTMLButtonElement).disabled).toBe(true);
});
