import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Setup from './SetupHarness';

afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });
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
  expect(await screen.findByRole('heading', { name: "The AI couldn't connect" })).toBeTruthy();
  const row = screen.getByText('Choose the AI your agent uses').closest('li')!;
  expect(within(row).getByRole('button', { name: 'Reset AI connection' })).toBeTruthy();
  expect(screen.getByText('Your workspace is ready on this computer.')).toBeTruthy();
  await user.click(within(row).getByRole('button', { name: 'Show details' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  const place = screen.getByRole('region', { name: 'This computer' });
  expect(within(place).getByRole('button', { name: 'Reset AI connection' })).toBeTruthy();
  expect(within(place).getByRole('button', { name: 'Hide details' })).toBeTruthy();
  expect(within(place).getByText('Workspace running')).toBeTruthy();
  await user.click(within(place).getByRole('button', { name: 'Reset AI connection' }));
  await user.click(within(place).getByRole('button', { name: 'Reset' }));
  if (destination === 'navigate') {
    await user.click(within(place).getByRole('button', { name: 'Connect the AI' }));
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
