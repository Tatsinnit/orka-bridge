import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../App';
import { SetupProgress, SetupSteps } from './Steps';
import { setupView } from './progress';
import type { SetupStatus } from './state';

const ready: SetupStatus = { selected: 'localVm', localVm: 'ready', model: 'available', modelSupported: true };
const actions = { busy: false, stopping: false, onCancel() {}, onRestart() {}, onChange() {}, onOpenExample() {} };
beforeEach(() => localStorage.setItem('bridge.welcomeSeen.v1', '1'));
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

test('a failed test stays red in its row while the workspace remains Ready', () => {
  expect(setupView(ready, { model: 'connected', check: 'failed' })).toMatchObject({ done: true, tone: 'bad', line: 'Its test failed' });
  render(<SetupProgress {...actions} status={ready} modelState="connected" firstCheck="failed" firstTestResult={{ failed: 1 }} />);
  expect(screen.getByText('Its test failed', { selector: '.step-result' }).closest('li')?.className).toBe('step-failed');
  expect(screen.getByText('Ready', { selector: '.pill' })).toBeTruthy();
});

test.each([['removalFailed', "Couldn't remove"], ['resetFailed', "Couldn't reset"]] as const)('AI %s names the failed cleanup action, not startup', (modelState, label) => {
  render(<SetupSteps {...actions} status={ready} modelState={modelState} />);
  const row = screen.getByText('Connect the AI', { selector: 'li b' }).closest('li')!;
  expect(within(row).getByText(label)).toBeTruthy();
});

test('returning connected/no-evidence is finished without claiming tests passed; pending first test is not', () => {
  expect(setupView(ready, { model: 'connected' })).toMatchObject({ done: true, line: 'Workspace ready', tone: 'ready' });
  expect(setupView(ready, { model: 'connected', initialAutomatic: true })).toMatchObject({ done: false, step: 4 });
});

test('completed and failed steps keep the same four rows', () => {
  const view = render(<SetupSteps {...actions} status={ready} modelState="connected" firstCheck="passed" />);
  const list = screen.getByRole('list', { name: 'Setup steps' });
  const rows = within(list).getAllByRole('listitem');
  expect(rows).toHaveLength(4);
  expect(rows.every(row => row.className === 'step-done')).toBe(true);
  view.rerender(<SetupSteps {...actions} status={{ ...ready, localVm: 'failed', workspaceFailure: { operation: 'download', cause: 'network', codes: [] } }} />);
  expect(within(list).getAllByRole('listitem')).toEqual(rows);
  expect(rows[1].querySelector('[role="alert"]')).toBeTruthy();
});

test('an unreadable poll preserves the last-known failed step without admitting recovery actions', () => {
  render(<SetupSteps {...actions} status={{ ...ready, localVm: 'failed', workspaceFailure: { operation: 'download', cause: 'network', codes: [] } }} error="Status unavailable" />);
  const list = screen.getByRole('list', { name: 'Setup steps' });
  expect(within(list).getAllByRole('listitem')).toHaveLength(4);
  expect(within(list).getByText("Last known: Couldn't start")).toBeTruthy();
  expect(within(list).getByRole('button', { name: 'Try again' }).hasAttribute('disabled')).toBe(true);
  expect(list.querySelector('.status-running, .status-passed, [aria-current]')).toBeNull();
});

test('unfinished local row, shell and card agree; finished workspace and cluster navigation offer no normal Stop', async () => {
  vi.stubGlobal('isTauri', true);
  const status: SetupStatus = ready;
  const ipc = vi.fn((command: string) => {
    if (command === 'setup_status') return status;
    if (command === 'setup_requirements') return null;
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'cluster_places') return { selectedKey: null, places: [], persistenceState: 'complete' };
    if (command === 'cluster_contexts') return [];
    throw Error('Unexpected ' + command);
  });
  mockIPC(ipc); const user = userEvent.setup(); render(<App />);
  const row = await screen.findByRole('region', { name: 'This computer' });
  expect(row.querySelector('.pill')?.className).toContain('status-needs-you');
  expect(within(row).queryByText('Running')).toBeNull();
  await user.click(within(row).getByRole('button', { name: 'Connect the AI' }));
  expect(within(screen.getByRole('navigation')).getByText('Setting up, step 3 of 4')).toBeTruthy();
  expect(screen.getByText('Connect the AI', { selector: 'li b' }).closest('li')?.getAttribute('aria-current')).toBe('step');
  expect(within(screen.getByRole('contentinfo')).getByText('Connect the AI, step 3 of 4')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Connect the AI' }).className).toBe('button button-primary');
  expect(screen.queryByRole('button', { name: 'Change where agents run' })).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect(await screen.findByRole('heading', { name: 'This computer', level: 1 })).toBeTruthy();
  expect(screen.queryByText(/Setting up, step 4 of 4/)).toBeNull();
  expect(screen.queryByRole('button', { name: /Stop.*workspace/ })).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  expect(await screen.findByRole('button', { name: 'kubectl context' })).toBeTruthy();
  expect(ipc.mock.calls.some(([cmd]) => ['setup_cancel_local', 'setup_change_local'].includes(cmd))).toBe(false);
  expect(screen.getByRole('contentinfo').textContent?.trim()).not.toBe('');
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(within(screen.getByRole('region', { name: 'This computer' })).getByRole('button', { name: 'Manage' }));
  expect(await screen.findByRole('heading', { name: 'This computer', level: 1 })).toBeTruthy();
  expect(screen.queryByRole('button', { name: /Stop.*workspace|^Start$|Continue/ })).toBeNull();
  expect(within(screen.getByRole('region', { name: 'Workspace setup' })).getByText('Ready', { selector: '.pill' }).className).toContain('status-passed');
  expect(within(screen.getByRole('contentinfo')).getByText('Workspace ready')).toBeTruthy();
  expect(ipc.mock.calls.some(([cmd]) => ['setup_cancel_local', 'setup_change_local', 'setup_select_local'].includes(cmd))).toBe(false);
});
