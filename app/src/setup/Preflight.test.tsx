import { openLocalSetup } from './test-navigation';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Setup from './Setup';
import App from '../App';

const requirements = { memoryBytes: 6442450944, imageBytes: 3651994962, stateDiskBytes: 34359738368 };
const ready = { platform: 'windows', virtualization: 'ready', memory: 'ready', installedMemoryBytes: 8589934592, requiredMemoryBytes: 6442450944 };
async function native(preflight: unknown = ready, integrated = true, onPlace = vi.fn()) {
  vi.stubGlobal('isTauri', true);
  localStorage.setItem('bridge.welcomeSeen.v1', '1');
  let value = preflight; let started = false;
  const status = () => ({ selected: started ? 'localVm' : null, localVm: started ? 'starting' : integrated ? 'idle' : 'notIntegrated', model: 'locked' });
  const ipc = vi.fn(command => {
    if (command === 'setup_status') return status();
    if (command === 'setup_requirements') return integrated ? requirements : null;
    if (command === 'setup_preflight') return value;
    if (command === 'setup_select_local') { started = true; return status(); }
    if (command === 'setup_open_help') return Promise.reject('SECRET_MARKER');
    if (command === 'cluster_contexts') return [];
    throw Error('unexpected command');
  });
  mockIPC(ipc);
  render(<Setup onOpenExample={() => {}} onPlace={onPlace} />); await openLocalSetup();
  return { ipc, onPlace, update: (next: unknown) => { value = next; } };
}
afterEach(() => { cleanup(); clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

test('the default local choice keeps Continue inside the containing card', async () => {
  await native();
  const action = await screen.findByRole('button', { name: 'Continue' });
  const card = action.closest('.card');
  expect(card).toBeTruthy();
  expect(screen.getByRole('heading', { name: 'Where should your agents run?' })).toBeTruthy();
  expect(within(card as HTMLElement).getByText('Step 1 of 4')).toBeTruthy();
  expect(within(card as HTMLElement).getByRole('group', { name: 'Where agents run' })).toBeTruthy();
  expect(screen.getByRole<HTMLInputElement>('radio', { name: 'On this computer' }).checked).toBe(true);
  expect(screen.getByRole<HTMLInputElement>('radio', { name: 'On a cluster you already have' }).checked).toBe(false);
  expect(screen.getByRole('button', { name: 'Check again' }).parentElement).toBe(action.parentElement);
  expect(screen.getByRole('button', { name: 'Copy details for help' }).parentElement).toBe(action.parentElement);
  const inside = within(card as HTMLElement).getByRole('button', { name: "What's inside" });
  expect(inside.compareDocumentPosition(action) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

test.each([
  ['ready', ready],
  ['blocked', { ...ready, virtualization: 'disabled' }],
  ['pending', new Promise(() => {})],
])('a %s local check cannot block cluster Continue or publish a destination from the radio choice', async (_name, observation) => {
  const { ipc, onPlace } = await native(observation);
  const user = userEvent.setup();
  const reads = ipc.mock.calls.filter(([c]) => c === 'setup_preflight').length;
  const local = screen.getByRole<HTMLInputElement>('radio', { name: 'On this computer' });
  local.focus();
  await user.keyboard('{ArrowDown}');
  expect(screen.getByRole<HTMLInputElement>('radio', { name: 'On a cluster you already have' }).checked).toBe(true);
  expect(screen.queryByRole('region', { name: 'Computer requirements' })).toBeNull();
  expect(screen.queryByRole('combobox', { name: 'kubectl context' })).toBeNull();
  expect(onPlace).not.toHaveBeenCalled();
  expect(ipc.mock.calls.some(([c]) => ['setup_select_local', 'cluster_admit', 'cluster_contexts'].includes(c))).toBe(false);
  const next = screen.getByRole('button', { name: 'Continue' });
  expect(next.matches(':disabled')).toBe(false);
  await user.click(next);
  expect(await screen.findByRole('combobox', { name: 'kubectl context' })).toBeTruthy();
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_preflight')).toHaveLength(reads);
  expect(ipc.mock.calls.some(([c]) => ['setup_select_local', 'cluster_admit'].includes(c))).toBe(false);
  expect(onPlace).not.toHaveBeenCalled();
});

test('returning to local while its check is pending cannot start early or accept an unmounted reply', async () => {
  let complete!: (value: unknown) => void;
  const { ipc, update, onPlace } = await native(new Promise(resolve => { complete = resolve; }));
  const user = userEvent.setup();
  await user.click(screen.getByRole('radio', { name: 'On a cluster you already have' }));
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(false);
  update(new Promise(() => {}));
  await user.click(screen.getByRole('radio', { name: 'On this computer' }));
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(true);
  complete(ready);
  await waitFor(() => expect(screen.getByRole('region', { name: 'Computer requirements' }).getAttribute('aria-busy')).toBe('true'));
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  expect(ipc.mock.calls.some(([c]) => c === 'setup_select_local')).toBe(false);
  expect(onPlace).not.toHaveBeenCalled();
});

test('insufficient installed memory blocks preparation, explains RAM, and can be rechecked without starting', async () => {
  const { ipc, update } = await native({ ...ready, memory: 'insufficient', installedMemoryBytes: 4294967296 });
  const user = userEvent.setup();
  const panel = await screen.findByRole('region', { name: 'Computer requirements' });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(true));
  const memory = within(panel).getByRole('region', { name: 'Installed memory' });
  await waitFor(() => expect(memory.textContent).toContain('about 4.3 GB installed; about 6.4 GB needed'));
  await user.click(within(memory).getByRole('button', { name: 'About this check' }));
  expect(memory.textContent).toMatch(/Closing apps does not increase installed memory/);
  expect(memory.textContent).toMatch(/cluster/);
  expect(ipc.mock.calls.some(([c]) => c === 'setup_select_local')).toBe(false);
  expect(within(panel).getByRole('status').textContent).toBe('Fix the computer requirements above before continuing.');
  expect(memory.querySelector('.status-failed')).toBeTruthy();
  update(ready);
  await user.click(screen.getByRole('button', { name: 'Check again' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(false));
  expect(within(panel).getByRole('status').textContent).toBe('These checks do not change this computer.');
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_preflight')).toHaveLength(2);
  expect(ipc.mock.calls.some(([c]) => c === 'setup_select_local')).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  expect(await screen.findByText('Starting the local cluster…')).toBeTruthy();
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_select_local')).toHaveLength(1);
});

test('rounded RAM values are approximate and the memory disclosure preserves exact failed amounts', async () => {
  const { ipc } = await native({ ...ready, memory: 'insufficient', installedMemoryBytes: 6400000000 });
  const memory = await screen.findByRole('region', { name: 'Installed memory' });
  await waitFor(() => expect(memory.textContent).toContain('about 6.4 GB installed; about 6.4 GB needed'));
  expect(screen.getByText(/about 6.4 GB of installed memory/)).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(true);
  await userEvent.setup().click(within(memory).getByRole('button', { name: 'About this check' }));
  expect(memory.textContent).toContain(`Required: ${requirements.memoryBytes.toLocaleString()} bytes.`);
  expect(memory.textContent).toContain(`Installed: ${(6400000000).toLocaleString()} bytes.`);
  expect(ipc.mock.calls.some(([c]) => c === 'setup_select_local')).toBe(false);
});

test('virtualization failure offers native fixed-topic help, sanitized copy, and a usable cluster alternative', async () => {
  const { ipc } = await native({ ...ready, virtualization: 'disabled' });
  const user = userEvent.setup();
  const virtualization = await screen.findByRole('region', { name: 'Virtualization' });
  await user.click(within(virtualization).getByRole('button', { name: 'Show me how' }));
  expect(ipc.mock.calls.find(([c]) => c === 'setup_open_help')).toEqual(['setup_open_help', { topic: 'virtualization' }]);
  expect(await screen.findByText(/Couldn't open help/)).toBeTruthy();
  await user.click(within(virtualization).getByRole('button', { name: 'Show me how' }));
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_open_help')).toHaveLength(1);
  expect(within(virtualization).getByRole('button', { name: 'Show me how' }).getAttribute('aria-expanded')).toBe('false');
  const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  await user.click(screen.getByRole('button', { name: 'Copy details for help' }));
  expect(copy.mock.calls[0][0]).toContain('disabled');
  expect(copy.mock.calls[0][0]).not.toMatch(/SECRET_MARKER|C:\\/);
  await user.click(screen.getByRole('button', { name: 'Back to clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  expect(await screen.findByLabelText('kubectl context')).toBeTruthy();
  expect(ipc.mock.calls.some(([c]) => c === 'setup_select_local')).toBe(false);
});

test.each([null, { ...ready, extra: 'SECRET_MARKER' }, { ...ready, requiredMemoryBytes: 1 }, { ...ready, installedMemoryBytes: -1 }, { ...ready, memory: 'SECRET_MARKER' }])('unreadable observations settle to unknown and defer to native launch: %j', async value => {
  const { ipc } = await native(value);
  expect(await screen.findAllByText('Not checked')).toHaveLength(2);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(false));
  expect(document.body.textContent).not.toContain('SECRET_MARKER');
  const next = screen.getByRole('button', { name: 'Continue' });
  const retry = screen.getByRole('button', { name: 'Check again' });
  expect(retry.className).toBe('button button-secondary');
  expect(next.className).toBe('button button-primary');
  expect(next.compareDocumentPosition(retry) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(screen.getByText('Bridge checks this when it starts.')).toBeTruthy();
  for (const name of ['Virtualization', 'Installed memory']) {
    const row = screen.getByRole('region', { name });
    expect(row.querySelector('.status-not-started')).toBeTruthy();
    expect(within(row).getByRole('button', { name: 'About this check' }).getAttribute('aria-expanded')).toBe('false');
  }
  await userEvent.setup().click(next);
  expect(await screen.findByText('Starting the local cluster…')).toBeTruthy();
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_select_local')).toHaveLength(1);
});

test('Mac unsupported virtualization explains platform support without Windows help', async () => {
  const { ipc } = await native({ ...ready, platform: 'macos', virtualization: 'unsupported' });
  const user = userEvent.setup();
  const virtualization = await screen.findByRole('region', { name: 'Virtualization' });
  await user.click(within(virtualization).getByRole('button', { name: 'About this check' }));
  expect(virtualization.textContent).toMatch(/Mac|cluster/);
  expect(ipc.mock.calls.some(([c]) => c === 'setup_open_help')).toBe(false);
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(true);
});

test('unavailable builds never query or claim host readiness', async () => {
  const { ipc } = await native(ready, false);
  await screen.findByText(/Local cluster setup is not included/);
  expect(screen.queryByRole('region', { name: 'Computer requirements' })).toBeNull();
  expect(ipc.mock.calls.some(([c]) => c === 'setup_preflight')).toBe(false);
});

test('a pending refresh disables Continue and an old request cannot survive leaving the local choice', async () => {
  let complete!: (value: unknown) => void;
  const { ipc } = await native(new Promise(resolve => { complete = resolve; }));
  const user = userEvent.setup();
  await screen.findByRole('region', { name: 'Computer requirements' });
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Back to clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  complete({ ...ready, virtualization: 'disabled' });
  await screen.findByLabelText('kubectl context');
  expect(screen.queryByRole('region', { name: 'Computer requirements' })).toBeNull();
  expect(ipc.mock.calls.some(([c]) => c === 'setup_select_local')).toBe(false);
});

test('preflight cause stays specific in the app status and host pill', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => command === 'setup_status' ? { selected: 'localVm', localVm: 'failed', model: 'locked', preflightFailure: 'insufficientMemory', localVmError: 'SECRET_MARKER' } : command === 'setup_requirements' ? requirements : Promise.reject({ kind: 'unavailable' }));
  render(<App />); await openLocalSetup();
  await waitFor(() => expect(screen.getByRole('contentinfo').textContent).toContain('less installed memory'));
  expect(document.querySelector('.host-chip')?.getAttribute('title')).toContain('less installed memory');
  expect(document.body.textContent).not.toContain('SECRET_MARKER');
});

test('a direct native preflight failure is decoded and explained without raw diagnostic text', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => command === 'setup_status' ? { selected: 'localVm', localVm: 'failed', model: 'locked', preflightFailure: 'insufficientMemory', localVmError: 'SECRET_MARKER RuntimeError::Preflight' } : command === 'setup_requirements' ? requirements : null);
  render(<Setup onOpenExample={() => {}} />);
  expect(await screen.findByText(/less installed memory/)).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy();
  expect(document.body.textContent).not.toContain('SECRET_MARKER');
});
