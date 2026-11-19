import { invoke } from '@tauri-apps/api/core';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import App from '../App';
import { openLocalSetup } from './test-navigation';
import { noUpgrade } from './test-upgrade';

const eligible = { state: 'eligible', transaction: null, createdAt: null, sourceReleases: ['0.1.2'], targetRelease: '0.1.3', retainedAllocatedBytes: 1200000000, additionalBytes: 2400000000, credential: 'eligible', canCancel: false, busy: false, error: null, repeated: false, canReplace: true, canRestore: false };
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });
test.each([false, true])('local Continue waits for upgrade knowledge before admitting ordinary setup (retained=%s)', async retained => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  let finish!: (value: unknown) => void;
  const pending = new Promise<unknown>(resolve => { finish = resolve; });
  const ipc = vi.fn((command: string): unknown => {
    if (command === 'workspace_upgrade_status') return pending;
    if (command === 'setup_status') return { selected: null, localVm: 'idle', model: 'locked' };
    if (command === 'setup_requirements') return null;
    if (command === 'cluster_contexts') return [];
    throw Error('Unexpected ' + command);
  });
  mockIPC(ipc); render(<App />); await openLocalSetup();
  await waitFor(() => expect(ipc.mock.calls.some(([command]) => command === 'setup_requirements')).toBe(true));
  await act(async () => { await Promise.resolve(); });
  const next = screen.getByRole('button', { name: 'Continue' });
  expect(next.matches(':disabled')).toBe(true);
  fireEvent.click(next);
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_select_local')).toEqual([]);
  await act(async () => { finish(retained ? eligible : noUpgrade); });
  if (retained) {
    await screen.findByRole('button', { name: 'Set up new workspace' });
    expect(next.matches(':disabled')).toBe(true);
  } else await waitFor(() => expect(next.matches(':disabled')).toBe(false));
  expect(ipc.mock.calls.filter(([command]) => ['setup_select_local', 'workspace_upgrade_replace', 'setup_connect_model', 'agent_run'].includes(command))).toEqual([]);
});
test.each([false, true])('retained eligibility survives an absent or cleared local choice and selection remains explicit (cleared=%s)', async cleared => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  let selected = cleared; let replaced = false;
  const ipc = vi.fn((command: string): unknown => {
    if (command === 'workspace_upgrade_status') return replaced ? { ...eligible, state: 'fresh', transaction: '10000000-0000-4000-8000-000000000001', createdAt: 1700000000, credential: 'absent', canReplace: false, canRestore: true } : eligible;
    if (command === 'workspace_upgrade_replace') {
      expect(localStorage.getItem('bridge.firstTest.v1')).not.toBe('pending');
      replaced = true;
      return { outcome: 'completed', status: { ...eligible, state: 'fresh', transaction: '10000000-0000-4000-8000-000000000001', createdAt: 1700000000, credential: 'absent', canReplace: false, canRestore: true } };
    }
    if (command === 'setup_change_local') { selected = false; return { selected: null, localVm: 'idle', model: 'locked' }; }
    if (command === 'setup_select_local') { expect(replaced).toBe(true); selected = true; return { selected: 'localVm', localVm: 'starting', model: 'locked', workspaceReplaced: true }; }
    if (command === 'setup_status') return { selected: selected ? 'localVm' : null, localVm: selected ? replaced ? 'starting' : 'failed' : 'idle', model: 'locked', ...(replaced ? { workspaceReplaced: true } : {}) };
    if (command === 'setup_requirements') return null;
    if (command === 'cluster_contexts') return [];
    throw Error('Unexpected ' + command);
  });
  localStorage.setItem('bridge.firstTest.v1', 'pending');
  mockIPC(ipc); render(<App />); await openLocalSetup();
  if (cleared) {
    await invoke('setup_change_local'); fireEvent(window, new Event('focus'));
    await screen.findByRole('heading', { name: 'Where should your agents run?' });
  }
  fireEvent.click(await screen.findByRole('button', { name: 'Set up new workspace' }));
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(true);
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['workspace_upgrade_replace', 'setup_select_local'].includes(c))).toEqual([]);
  fireEvent.click(screen.getByRole('button', { name: 'Not now' }));
  fireEvent.click(screen.getByRole('radio', { name: 'On a cluster you already have' }));
  fireEvent.click(screen.getByRole('radio', { name: 'On this computer' }));
  expect(screen.queryByRole('group', { name: 'Set up new workspace?' })).toBeNull();
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['workspace_upgrade_replace', 'setup_select_local'].includes(c))).toEqual([]);
  fireEvent.click(screen.getByRole('button', { name: 'Set up new workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  await screen.findByText(/New workspace selected/);
  const continueButton = screen.getByRole('button', { name: 'Continue' });
  await waitFor(() => expect(continueButton.matches(':disabled')).toBe(false));
  expect(ipc.mock.calls.map(([c]) => c).filter(c => c === 'setup_select_local')).toEqual([]);
  fireEvent.click(continueButton);
  await waitFor(() => expect(ipc.mock.calls.map(([c]) => c).filter(c => ['workspace_upgrade_replace', 'setup_select_local'].includes(c))).toEqual(['workspace_upgrade_replace', 'setup_select_local']));
  expect(localStorage.getItem('bridge.firstTest.v1')).toBe('attempted');
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['setup_connect_model', 'agent_run', 'agent_chat_start'].includes(c))).toEqual([]);
});

test('setup offers complete native eligibility instead of the disabled matching-version retry', async () => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1'); localStorage.setItem('bridge.firstTest.v1', 'pending');
  let replaced = false;
  const ipc = vi.fn((command: string) => {
    if (command === 'workspace_upgrade_status' || command === 'workspace_upgrade_replace') {
      if (command === 'workspace_upgrade_replace') replaced = true;
      const status = replaced ? { ...eligible, state: 'consumed', transaction: '10000000-0000-4000-8000-000000000001', createdAt: 1700000000, credential: 'eligible', canReplace: false, canRestore: true } : eligible;
      return command === 'workspace_upgrade_replace' ? { outcome: 'completed', status } : status;
    }
    if (command === 'setup_status') return { selected: 'localVm', localVm: replaced ? 'ready' : 'failed', model: replaced ? 'available' : 'locked', ...(replaced ? { workspaceReplaced: true } : { workspaceFailure: { cause: 'workspaceUpdateRequired', operation: 'verify', codes: [] } }) };
    if (command === 'setup_requirements') return null;
    if (command === 'cluster_contexts') return [];
    throw Error('Unexpected ' + command);
  });
  mockIPC(ipc); render(<App />); await openLocalSetup();
  await screen.findByRole('button', { name: 'Set up new workspace' });
  expect(document.querySelector('.setup-progress .workspace-upgrade')).not.toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Set up new workspace' }));
  expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  await screen.findByText(/New workspace selected/);
  await waitFor(() => expect(localStorage.getItem('bridge.firstTest.v1')).toBe('attempted'));
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['setup_select_local', 'setup_connect_model', 'agent_run'].includes(c))).toEqual([]);
  expect(screen.getByRole('button', { name: 'Restore previous workspace' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Use saved connection' })).toBeTruthy();
  expect(document.querySelector('.setup-model')).toBeNull();
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation(command => command === 'workspace_upgrade_status'
    ? { ...eligible, state: 'sourceSelected', transaction: '10000000-0000-4000-8000-000000000001', createdAt: 1700000000, credential: 'absent', canReplace: false, canRestore: true, busy: true, canCancel: true }
    : original(command));
  await waitFor(() => expect(document.querySelector('.steps-head .pill')?.textContent).toBe('Updating'), { timeout: 2000 });
  expect(document.querySelector('.steps-head .pill')?.classList.contains('status-running')).toBe(true);
  await waitFor(() => expect(screen.getByRole('contentinfo').textContent).toContain('Updating workspace'));
  ipc.mockImplementation(command => {
    if (command === 'workspace_upgrade_status') throw Error('Lost upgrade observation');
    return original(command);
  });
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Restore previous workspace' })).toBeNull(), { timeout: 2000 });
  expect(screen.queryByRole('button', { name: 'Use saved connection' })).toBeNull();
  expect(within(document.querySelector('.workspace-upgrade')!).getByText(/Workspace recovery needs attention/)).toBeTruthy();
});

test.each([false, true])('returning restore stays in Manage without ordinary restart or AI actions (busy=%s)', async busy => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1'); localStorage.setItem('bridge.firstTest.v1', 'attempted');
  const ipc = vi.fn((command: string) => {
    if (command === 'workspace_upgrade_status') return { ...eligible, state: 'restoring', transaction: '10000000-0000-4000-8000-000000000001', createdAt: 1700000000, credential: 'absent', canReplace: false, canRestore: true, busy, canCancel: busy };
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'failed', model: 'locked', workspaceReplaced: true };
    if (command === 'setup_requirements') return null;
    throw Error('Unavailable ' + command);
  });
  mockIPC(ipc); render(<App />); await openLocalSetup();
  await screen.findByRole('button', { name: busy ? 'Cancel update' : 'Restore previous workspace' });
  expect(document.querySelector('.returning-workspace .workspace-upgrade')).not.toBeNull();
  if (!busy) {
    await waitFor(() => expect(screen.getByRole('contentinfo').textContent).toContain('Workspace recovery needs attention'));
    expect(document.querySelector('.returning-workspace > [role="status"]')).toBeNull();
    expect(screen.getByRole('button', { name: 'Clusters' }).textContent).not.toContain('ready');
  }
  if (busy) {
    expect(document.querySelector('.returning-workspace .pill')?.textContent).toBe('Updating');
    expect(document.querySelector('.returning-workspace .pill')?.classList.contains('status-running')).toBe(true);
  }
  expect(screen.queryByRole('button', { name: 'Restart workspace' })).toBeNull();
  expect(screen.queryByRole('button', { name: /Set up.*workspace/ })).toBeNull();
  expect(document.querySelector('.setup-model')).toBeNull();
  expect(ipc.mock.calls.map(([command]) => command).filter(command => ['setup_select_local', 'setup_cancel_local', 'setup_connect_model', 'agent_run'].includes(command))).toEqual([]);
});
test.each(['eligible', 'fresh'] as const)('ready VM observations cannot advertise success for unresolved %s upgrade errors', async state => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1'); localStorage.setItem('bridge.firstTest.v1', 'attempted');
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return { ...eligible, state, error: 'network', ...(state === 'fresh' ? { transaction: '10000000-0000-4000-8000-000000000001', createdAt: 1700000000, canReplace: false, canRestore: true } : {}) };
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available', workspaceReplaced: true };
    if (command === 'setup_requirements') return null;
    throw Error('Unavailable ' + command);
  });
  render(<App />); await openLocalSetup();
  await waitFor(() => expect(screen.getByRole('contentinfo').textContent).toContain('Workspace update needs attention'));
  expect(document.querySelector('.returning-workspace .pill')?.textContent).toBe('Needs you');
  expect(screen.getByRole('button', { name: 'Clusters' }).textContent).not.toContain('ready');
});
test('ready VM observations cannot override recovery and consumption clears the recovery summary', async () => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1'); localStorage.setItem('bridge.firstTest.v1', 'attempted');
  let recovered = false;
  const ipc = vi.fn((command: string) => {
    if (command === 'workspace_upgrade_status') return { ...eligible, state: recovered ? 'consumed' : 'sourceSelected', transaction: '10000000-0000-4000-8000-000000000001', createdAt: 1700000000, canReplace: !recovered, canRestore: true };
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available', workspaceReplaced: true };
    if (command === 'setup_requirements') return null;
    throw Error('Unavailable ' + command);
  });
  mockIPC(ipc); render(<App />); await openLocalSetup();
  await waitFor(() => expect(screen.getByRole('contentinfo').textContent).toContain('Workspace recovery needs attention'));
  expect(document.querySelector('.returning-workspace > [role="status"]')).toBeNull();
  expect(screen.getByRole('button', { name: 'Clusters' }).textContent).not.toContain('ready');
  recovered = true; fireEvent(window, new Event('focus'));
  await screen.findByRole('button', { name: 'Use saved connection' });
  await waitFor(() => expect(screen.getByRole('contentinfo').textContent).toContain('Workspace ready'));
  expect(screen.getByRole('contentinfo').textContent).not.toContain('recovery');
  expect(ipc.mock.calls.map(([command]) => command).filter(command => ['setup_select_local', 'setup_connect_model', 'agent_run'].includes(command))).toEqual([]);
});
test.each([false, true])('unreadable workspace status takes precedence over busy upgrade presentation (returning=%s)', async returning => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  if (returning) localStorage.setItem('bridge.firstTest.v1', 'attempted');
  let unreadable = false;
  const ipc = vi.fn((command: string) => {
    if (command === 'workspace_upgrade_status') return { ...eligible, state: 'sourceSelected', transaction: '10000000-0000-4000-8000-000000000001', createdAt: 1700000000, credential: 'absent', canReplace: false, canRestore: false, busy: true, canCancel: true };
    if (command === 'setup_status') { if (unreadable) throw Error('Lost workspace observation'); return { selected: 'localVm', localVm: 'failed', model: 'locked', workspaceReplaced: true }; }
    if (command === 'setup_requirements') return null;
    throw Error('Unavailable ' + command);
  });
  mockIPC(ipc); render(<App />); await openLocalSetup();
  await screen.findByRole('button', { name: 'Cancel update' });
  await waitFor(() => expect(screen.getByRole('contentinfo').textContent).toContain('Updating workspace'));
  unreadable = true; fireEvent(window, new Event('focus'));
  await waitFor(() => expect(document.querySelector('.steps-head .pill')?.textContent).toBe('Status unavailable'));
  expect(screen.getByRole('contentinfo').textContent).toContain('Workspace unavailable');
  expect(screen.getByRole('contentinfo').textContent).not.toContain('Updating');
  if (returning) expect(document.querySelector('.returning-workspace > [role="status"]')?.textContent).toContain('Workspace status is unavailable');
  expect(ipc.mock.calls.map(([command]) => command).filter(command => ['setup_select_local', 'setup_connect_model', 'agent_run'].includes(command))).toEqual([]);
});
test('cold replacement status failure cannot advertise Connect before credential disposition is known', async () => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  const ipc = vi.fn((command: string) => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available', workspaceReplaced: true };
    if (command === 'setup_requirements') return null;
    if (command === 'cluster_contexts') return [];
    throw Error('Unavailable ' + command);
  });
  mockIPC(ipc); render(<App />); await openLocalSetup();
  await waitFor(() => expect(document.querySelector('.steps-head')).toBeTruthy());
  expect(document.querySelector('.setup-model')).toBeNull();
  expect(screen.getAllByText('Workspace update information is unavailable. Check again before changing your workspace.').length).toBeGreaterThan(0);
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['setup_connect_model', 'agent_run'].includes(c))).toEqual([]);
});
