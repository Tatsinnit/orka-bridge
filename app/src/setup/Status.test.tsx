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

test('a failed test is red in the shared presentation and local page, never Ready', () => {
  expect(setupView(ready, { model: 'connected', check: 'failed' })).toMatchObject({ done: true, tone: 'bad', line: 'Its test failed' });
  render(<SetupProgress {...actions} status={ready} modelState="connected" firstCheck="failed" firstTestResult={{ failed: 1 }} />);
  expect(screen.getByText('Its test failed', { selector: '.pill' }).className).toContain('status-failed');
  expect(screen.queryByText('Ready')).toBeNull();
});

test('returning connected/no-evidence is finished without claiming tests passed; pending first test is not', () => {
  expect(setupView(ready, { model: 'connected' })).toMatchObject({ done: true, line: 'Local cluster running', tone: 'wait' });
  expect(setupView(ready, { model: 'connected', initialAutomatic: true })).toMatchObject({ done: false, step: 5 });
});

test('completed steps collapse and a failure does not display four waiting rows', async () => {
  const user = userEvent.setup();
  const view = render(<SetupSteps {...actions} status={ready} modelState="connected" firstCheck="passed" />);
  expect(screen.getByText('Completed setup steps').closest('details')?.open).toBe(false);
  await user.click(screen.getByText('Completed setup steps'));
  expect(within(screen.getByRole('list', { name: 'Completed setup steps' })).getAllByRole('listitem')).toHaveLength(5);
  view.rerender(<SetupSteps {...actions} status={{ ...ready, localVm: 'failed', workspaceFailure: { operation: 'download', cause: 'network', codes: [] } }} />);
  expect(within(screen.getByRole('list', { name: 'Setup steps' })).getAllByRole('listitem')).toHaveLength(1);
});

test('an unreadable poll preserves the last-known failed step without admitting recovery actions', () => {
  render(<SetupSteps {...actions} status={{ ...ready, localVm: 'failed', workspaceFailure: { operation: 'download', cause: 'network', codes: [] } }} error="Status unavailable" />);
  const list = screen.getByRole('list', { name: 'Setup steps' });
  expect(within(list).getAllByRole('listitem')).toHaveLength(1);
  expect(within(list).getByText('Last known: Needs attention')).toBeTruthy();
  expect(within(list).getByRole('button', { name: 'Try again' }).hasAttribute('disabled')).toBe(true);
  expect(list.querySelector('.status-running, .status-passed, [aria-current]')).toBeNull();
});

test('unfinished local row, shell and card agree; finished local page owns Stop and cluster navigation never stops', async () => {
  vi.stubGlobal('isTauri', true);
  let selected: 'localVm' | null = 'localVm';
  const ipc = vi.fn((command: string) => {
    if (command === 'setup_status') return selected ? ready : { selected: null, localVm: 'cancelled', model: 'locked' };
    if (command === 'setup_requirements') return null;
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'setup_change_local') { selected = null; return { selected, localVm: 'cancelled', model: 'locked' }; }
    if (command === 'cluster_places') return { selectedKey: null, places: [] };
    if (command === 'cluster_contexts') return [];
    throw Error('Unexpected ' + command);
  });
  mockIPC(ipc); const user = userEvent.setup(); render(<App />);
  const row = await screen.findByRole('region', { name: 'This computer' });
  expect(row.querySelector('.pill')?.className).toContain('status-needs-you');
  expect(within(row).queryByText('Running')).toBeNull();
  await user.click(within(row).getByRole('button', { name: 'Connect the AI' }));
  expect(within(screen.getByRole('navigation')).getByText('Connect the AI')).toBeTruthy();
  expect(screen.getByText('Step 4 of 5')).toBeTruthy();
  expect(within(screen.getByRole('contentinfo')).getByText('Connect the AI')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Connect the AI' }).className).toBe('button button-primary');
  expect(screen.queryByRole('button', { name: 'Change where agents run' })).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect(await screen.findByRole('heading', { name: 'Local cluster', level: 1 })).toBeTruthy();
  expect(screen.queryByText(/Step 5 of 5/)).toBeNull();
  expect(screen.getByRole('button', { name: 'Stop the local cluster' }).className).toBe('button button-quiet');
  await user.click(screen.getByRole('button', { name: 'Back to clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  expect(await screen.findByRole('combobox', { name: 'kubectl context' })).toBeTruthy();
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'setup_change_local')).toBe(false);
  expect(screen.getByRole('contentinfo').textContent?.trim()).not.toBe('');
  await user.click(screen.getByRole('button', { name: 'Back to clusters' }));
  await user.click(within(screen.getByRole('region', { name: 'This computer' })).getByRole('button', { name: 'Manage' }));
  await user.click(screen.getByRole('button', { name: 'Stop the local cluster' }));
  expect(await screen.findByRole('button', { name: 'Continue' })).toBeTruthy();
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'setup_change_local')).toHaveLength(1);
});
