import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Setup from './SetupHarness';

afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

test('AI recovery stays usable after observed failure and restart without claiming a failed workspace is running', async () => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  let phase = 'ready';
  const status = () => ({ selected: 'localVm', localVm: phase, model: phase === 'ready' ? 'available' : 'locked' });
  const ipc = vi.fn((command: string) => {
    if (command === 'setup_status') return status();
    if (command === 'setup_requirements') return null;
    if (command === 'cluster_places') return { places: [], selectedKey: null, persistenceState: 'complete' };
    if (command === 'setup_select_local') { phase = 'ready'; return status(); }
    if (command === 'setup_connect_model') throw { kind: 'modelInstallationMismatch', recovery: {
      installation: '10000000-0000-4000-8000-000000000001', savedInstallation: '20000000-0000-4000-8000-000000000002', credentialSlot: '30000000-0000-4000-8000-000000000003',
    } };
    if (command === 'setup_reset_model') return { ready: false };
    throw { kind: 'unavailable' };
  });
  mockIPC(ipc); const user = userEvent.setup(); render(<Setup onOpenExample={() => {}} />);
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await screen.findByRole('region', { name: 'Saved AI connection' });
  phase = 'failed'; fireEvent.focus(window);
  await screen.findByRole('alert');
  expect(screen.queryByText('Your workspace is running. Nothing is lost.')).toBeNull();
  await user.click(await screen.findByRole('button', { name: 'Try again' }));
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  const recovery = await screen.findByRole('region', { name: 'Saved AI connection' });
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('This computer');
  await user.click(within(recovery).getByRole('button', { name: 'Reset AI connection' }));
  await user.click(screen.getByRole('button', { name: 'Reset' }));
  await screen.findByText('Connection reset. Choose the AI again.');
  expect(ipc.mock.calls.filter(([name]) => name === 'setup_reset_model')).toHaveLength(1);
  expect(ipc.mock.calls.filter(([name]) => name === 'setup_change_local')).toHaveLength(0);
});
test.each(['stay', 'navigate'])('AI recovery keeps reset completion visible after %s on the running-place card', async destination => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  let finish!: (value: unknown) => void;
  const ipc = vi.fn((command: string) => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_requirements') return null;
    if (command === 'cluster_places') return { places: [], selectedKey: null, persistenceState: 'complete' };
    if (command === 'setup_connect_model') throw { kind: 'modelInstallationMismatch', recovery: {
      installation: '10000000-0000-4000-8000-000000000001', savedInstallation: '20000000-0000-4000-8000-000000000002', credentialSlot: '30000000-0000-4000-8000-000000000003',
    } };
    if (command === 'setup_reset_model') return new Promise(resolve => { finish = resolve; });
    throw { kind: 'unavailable' };
  });
  mockIPC(ipc); const user = userEvent.setup(); render(<Setup onOpenExample={() => {}} />);
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' })); // Places' navigation action
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  expect(await screen.findByRole('heading', { level: 1, name: 'This computer' })).toBeTruthy();
  expect(screen.getByText("The AI couldn't connect", { selector: 'p' })).toBeTruthy();
  const row = screen.getByText('Connect the AI', { selector: 'li b' }).closest('li')!;
  expect(within(row).getByRole('button', { name: 'Reset AI connection' })).toBeTruthy();
  expect(screen.getByText('Your workspace is ready on this computer.')).toBeTruthy();
  await user.click(within(row).getByRole('button', { name: 'Show details' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  const place = screen.getByRole('region', { name: 'This computer' });
  expect(within(place).getByRole('button', { name: 'Reset AI connection' })).toBeTruthy();
  expect(within(place).getByRole('button', { name: 'Hide details' })).toBeTruthy();
  expect(within(place).getByText('The AI needs to be set up again')).toBeTruthy();
  expect(within(place).getByText('Needs you', { selector: '.pill' })).toBeTruthy();
  expect(place.querySelectorAll('.button-primary')).toHaveLength(1);
  await user.click(within(place).getByRole('button', { name: 'Reset AI connection' }));
  await user.click(within(place).getByRole('button', { name: 'Reset' }));
  if (destination === 'navigate') {
    await user.click(within(place).getByRole('button', { name: 'View workspace' }));
    screen.getByRole('heading', { level: 1 }).focus();
  }
  const focused = document.activeElement;
  await act(async () => { finish({ ready: false }); });
  const area = destination === 'stay' ? within(place) : screen;
  expect(area.getByText('Connection reset. Choose the AI again.')).toBeTruthy();
  expect(area.getByRole('radio', { name: 'GitHub Copilot' }).matches(':disabled')).toBe(false);
  expect(ipc.mock.calls.filter(([name]) => name === 'setup_reset_model')).toHaveLength(1);
  if (destination === 'stay') expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('Clusters');
  else expect(document.activeElement).toBe(focused);
});
