import { openLocalSetup } from './test-navigation';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../App';
import Setup from './Setup';

function native() {
  vi.stubGlobal('isTauri', true);
  const selected = 'localVm';
  let stopped = false;
  const ipc = vi.fn((command: string) => {
    if (command === 'setup_status') return { selected, localVm: stopped ? 'cancelled' : 'ready', model: stopped ? 'locked' : 'available' };
    if (command === 'setup_requirements') return null;
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'setup_cancel_local') { stopped = true; return { selected, localVm: 'cancelled', model: 'locked' }; }
    if (command === 'cluster_contexts') return [];
    throw Error('Unexpected command: ' + command);
  });
  mockIPC(ipc); return ipc;
}
beforeEach(() => localStorage.setItem('bridge.welcomeSeen.v1', '1'));
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

test('AI connection is the only next primary and its prerequisite is visible before clicking', async () => {
  const ipc = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  const connect = await screen.findByRole('button', { name: 'Connect the AI' });
  expect(connect.className).toBe('button button-primary');
  expect(screen.getByText(/GitHub CLI must be installed and signed in/)).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Open example' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Change where agents run' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Stop the workspace' }).className).toBe('button button-secondary');
  await user.click(connect);
  expect(await screen.findByText('Connected', { selector: '.steps-head .pill' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Open your first agent' }).className).toBe('button button-primary');
  expect(document.body.textContent).not.toMatch(/guest|\bVM\b|inference/);
  expect(screen.getByText('gpt-5.6-terra', { selector: 'code' })).toBeTruthy();
  expect(screen.queryByText('Engine diagnostics')).toBeNull();
  expect(ipc.mock.calls.some(([command]) => command === 'engine_call')).toBe(false);
});

test.each(['Run it again', 'See what it answered'])('%s selects this computer before its guarded local handoff, without starting a VM', async action => {
  const ipc = native(); const user = userEvent.setup(); const calls: string[] = [];
  render(<Setup place={{ kind: 'cluster' }} firstCheck="stale" firstTestResult={{ failed: 0 }}
    onPlace={place => calls.push(place.kind)} onOpenExample={() => calls.push('Designer')} />);
  await openLocalSetup(user);
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  expect(calls).toEqual(['thisComputer']);
  calls.length = 0;
  await user.click(await screen.findByRole('button', { name: action }));
  expect(calls).toEqual(['thisComputer', 'Designer']);
  expect(ipc.mock.calls.map(([command]) => command)).not.toContain('setup_select_local');
});

test('the restored local page owns Stop and stays restartable even without an old welcome preference', async () => {
  localStorage.removeItem('bridge.welcomeSeen.v1');
  const ipc = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  const stop = await screen.findByRole('button', { name: 'Stop the workspace' });
  expect(stop.className).toBe('button button-secondary');
  await user.click(stop);
  await user.click(screen.getByRole('button', { name: 'Stop workspace' }));
  expect(await screen.findByRole('button', { name: 'Start' })).toBeTruthy();
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_cancel_local')).toHaveLength(1);
});

test('cluster picking is explicitly separate from stopping and does not stop the local workspace', async () => {
  const ipc = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  expect(await screen.findByRole('button', { name: 'kubectl context' })).toBeTruthy();
  expect(ipc.mock.calls.some(([command]) => command === 'setup_change_local')).toBe(false);
});
