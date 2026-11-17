import { openLocalSetup } from './test-navigation';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../App';

const preference = 'bridge.welcomeSeen.v1';
const unselected = { selected: null, localVm: 'idle', model: 'locked' };
const requirements = { memoryBytes: 6442450944, imageBytes: 3651994962, stateDiskBytes: 34359738368, downloadBytes: 2800000000 };
function native(initial: object = unselected, sizing: unknown = requirements) {
  vi.stubGlobal('isTauri', true);
  let status = initial;
  const ipc = vi.fn((command: string) => {
    if (command === 'setup_status') return status;
    if (command === 'setup_requirements') return sizing;
    if (command === 'setup_select_local') {
      status = { selected: 'localVm', localVm: 'starting', model: 'locked' };
      return status;
    }
    throw new Error('Unsupported command');
  });
  mockIPC(ipc);
  return ipc;
}
beforeEach(() => { localStorage.clear(); });
afterEach(() => { cleanup(); clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

test('Get started remembers the welcome without starting a workspace, including on remount', async () => {
  const ipc = native(); const user = userEvent.setup(); const first = render(<App />);
  expect(await screen.findByRole('heading', { name: 'Build an AI agent that does a real job' })).toBeTruthy();
  expect(screen.getByText('Write it')).toBeTruthy();
  expect(screen.getByText('Test it')).toBeTruthy();
  expect(screen.getByText('Share it')).toBeTruthy();
  expect(screen.getByText('About 5 minutes, mostly a download').parentElement).toBe(screen.getByRole('button', { name: 'Get started' }).parentElement);
  expect(screen.getByText(/Team clusters are early in this preview/).textContent).toContain('Send');
  await user.click(screen.getByRole('button', { name: 'Get started' }));
  const heading = await screen.findByRole('heading', { name: 'Clusters' });
  expect(document.activeElement).toBe(heading);
  expect(localStorage.getItem(preference)).toBe('1');
  first.unmount(); render(<App />);
  expect(await screen.findByRole('button', { name: 'Set up' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Get started' })).toBeNull();
  expect(ipc.mock.calls.some(([command]) => command === 'setup_select_local')).toBe(false);
});

test('welcome chrome advances only after Get started, including with denied preference storage', async () => {
  native(); vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new DOMException('Denied', 'SecurityError'); });
  const user = userEvent.setup(); render(<App />);
  expect(await within(screen.getByRole('navigation')).findByText('Start here')).toBeTruthy();
  expect(within(screen.getByRole('contentinfo')).getByText('Nothing is set up yet')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Get started' }));
  expect(within(screen.getByRole('navigation')).getByText('Choose where agents run')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect(document.querySelectorAll('#page-title')).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(await screen.findByRole('button', { name: 'Set up' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Get started' })).toBeNull();
});

test('a saved native choice skips welcome even without a renderer preference', async () => {
  const ipc = native({ selected: 'localVm', localVm: 'starting', model: 'locked' }); render(<App />);
  expect(await screen.findByText('Starting the local cluster…')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Get started' })).toBeNull();
  expect(ipc.mock.calls.some(([command]) => command === 'setup_select_local')).toBe(false);
});

test('expert route is available without starting a local workspace', async () => {
  const ipc = native(); const user = userEvent.setup(); render(<App />);
  const expert = await screen.findByRole('button', { name: 'I already run Orka on a cluster' });
  expect(expert.matches(':disabled')).toBe(false);
  expect(expert.classList.contains('welcome-expert')).toBe(true);
  await user.click(expert);
  expect(await screen.findByRole('button', { name: 'kubectl context' })).toBeTruthy();
  expect(ipc.mock.calls.some(([command]) => command === 'setup_select_local')).toBe(false);
});

test('keyboard disclosure and opening local setup are inert; only Continue starts once', async () => {
  localStorage.setItem(preference, '1'); const ipc = native(); const user = userEvent.setup(); render(<App />);
  const local = await screen.findByRole('button', { name: 'Set up' });
  local.focus(); await user.keyboard(' ');
  const disclosure = screen.getByRole('button', { name: "What's inside" });
  const details = document.getElementById(disclosure.getAttribute('aria-controls')!);
  expect(details).not.toBeNull();
  expect(details?.hasAttribute('hidden')).toBe(true);
  disclosure.focus(); await user.keyboard('{Enter}');
  expect(disclosure.getAttribute('aria-expanded')).toBe('true');
  expect(screen.getByText(/local cluster includes Orka/)).toBeTruthy();
  expect(ipc.mock.calls.some(([command]) => command === 'setup_select_local')).toBe(false);
  const next = screen.getByRole('button', { name: 'Continue' });
  next.focus(); await user.keyboard('{Enter}{Enter}');
  expect(await screen.findByText('Starting the local cluster…')).toBeTruthy();
  expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'This computer' }));
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_select_local')).toEqual([['setup_select_local', {}]]);
});

test('requirements render the native build values rather than fixed image or memory sizes', async () => {
  localStorage.setItem(preference, '1'); native(unselected, { memoryBytes: 8589934592, imageBytes: 18253611008, stateDiskBytes: 68719476736, downloadBytes: 2380070756 });
  render(<App />); await openLocalSetup();
  expect(screen.getByText('Downloads about 2.4 GB. Needs virtualization and 8.6 GB of memory. Data disk can grow to 68.7 GB.')).toBeTruthy();
  expect(screen.getByText('Use Orka on a Kubernetes cluster you already have.')).toBeTruthy();
  expect(screen.getAllByText(/step 1 of 4/i)).toHaveLength(1);
  expect(within(screen.getByRole('contentinfo')).queryByText(/step 1/)).toBeNull();
  await userEvent.setup().click(screen.getByRole('button', { name: "What's inside" }));
  const imageDetails = screen.getByText(/image size is 18.3 GB/);
  expect(imageDetails).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(false);
  // WebKit exposes interpolated sibling text nodes as separate AXStaticText values.
  expect(imageDetails.childNodes).toHaveLength(1);
  expect(screen.queryByText(/GiB|free disk|18.3 GB download/)).toBeNull();
});

test('debug requirements without a download retain RAM checks and disk facts without inventing a transfer', async () => {
  localStorage.setItem(preference, '1'); native(unselected, { ...requirements, downloadBytes: null });
  render(<App />); await openLocalSetup();
  expect(await screen.findByText('Needs virtualization and 6.4 GB of memory. Data disk can grow to 34.4 GB.')).toBeTruthy();
  expect(screen.queryByText(/Downloads about/)).toBeNull();
  expect(screen.getByRole('region', { name: 'Computer requirements' })).toBeTruthy();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(false));
});

test('release welcome exposes unavailability before Get started without replacing the workflow copy', async () => {
  const ipc = native({ ...unselected, localVm: 'notIntegrated' }, null); render(<App />);
  expect(await screen.findByRole('button', { name: 'Get started' })).toBeTruthy();
  expect(screen.getByText('Local cluster setup is not included in this build. Nothing will be downloaded or started.')).toBeTruthy();
  expect(screen.getByText('Share it')).toBeTruthy();
  expect(ipc.mock.calls.some(([command]) => command === 'setup_select_local')).toBe(false);
});

test('release-unavailable setup stays explicit and does not invent requirements', async () => {
  localStorage.setItem(preference, '1'); native({ ...unselected, localVm: 'notIntegrated' }, null); render(<App />);
  expect(await screen.findByText('Local cluster setup is not included in this build. Nothing will be downloaded or started.')).toBeTruthy();
  expect(screen.queryByText(/GiB/)).toBeNull();
});

test.each(['getItem', 'setItem'] as const)('denied preference %s does not prevent setup', async method => {
  native(); vi.spyOn(Storage.prototype, method).mockImplementation(() => { throw new DOMException('Denied', 'SecurityError'); });
  const user = userEvent.setup(); render(<App />);
  await user.click(await screen.findByRole('button', { name: 'Get started' }));
  expect(await screen.findByRole('button', { name: 'Set up' })).toBeTruthy();
});

test.each([
  { ...requirements, imageBytes: -1 },
  { ...requirements, memoryBytes: Number.MAX_SAFE_INTEGER + 1 },
  { ...requirements, stateDiskBytes: 0 },
  { ...requirements, downloadBytes: 0 },
  { ...requirements, downloadBytes: 'SECRET_MARKER' },
  { memoryBytes: 'SECRET_MARKER', imageBytes: 123, stateDiskBytes: 123 },
  { ...requirements, secret: 'SECRET_MARKER' },
])('unsupported sizing cannot produce requirements copy or leak data: %j', async value => {
  localStorage.setItem(preference, '1'); native(unselected, value); render(<App />); await openLocalSetup();
  expect(await screen.findByText(/Requirements are not available yet for this build\./)).toBeTruthy();
  await userEvent.setup().click(screen.getByRole('button', { name: "What's inside" }));
  expect(document.body.textContent).not.toMatch(/\d[.,]\d GB|GiB/);
  expect(document.body.textContent).not.toContain('SECRET_MARKER');
});

test('an unseen welcome cannot hide a failed status read or overwrite its preference', async () => {
  native(); let failed = true;
  const ipc = vi.fn(command => command === 'setup_status'
    ? failed ? Promise.reject('SECRET_MARKER') : { selected: 'localVm', localVm: 'starting', model: 'locked' }
    : command === 'setup_requirements' ? requirements : Promise.reject({ kind: 'unavailable' }));
  mockIPC(ipc); const user = userEvent.setup(); render(<App />);
  expect(await screen.findByRole('alert')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Get started' })).toBeNull();
  expect(localStorage.getItem(preference)).toBeNull();
  failed = false;
  await user.click(screen.getByRole('button', { name: 'Check again' }));
  expect(await screen.findByText('Starting the local cluster…')).toBeTruthy();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(localStorage.getItem(preference)).toBeNull();
  expect(ipc.mock.calls.some(([command]) => command === 'setup_select_local')).toBe(false);
});

test('a delayed initial status cannot flash welcome over an existing workspace', async () => {
  native(); let complete!: (value: unknown) => void;
  mockIPC(command => command === 'setup_status' ? new Promise(resolve => { complete = resolve; })
    : command === 'setup_requirements' ? requirements : Promise.reject({ kind: 'unavailable' }));
  render(<App />);
  expect(screen.queryByRole('button', { name: 'Get started' })).toBeNull();
  complete({ selected: 'localVm', localVm: 'starting', model: 'locked' });
  await waitFor(() => expect(screen.getByText('Starting the local cluster…')).toBeTruthy());
  expect(screen.queryByRole('button', { name: 'Get started' })).toBeNull();
});
