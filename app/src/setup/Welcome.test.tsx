import { noUpgrade } from './test-upgrade';
import { openLocalSetup } from './test-navigation';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../App';
import SetupHarness from './SetupHarness';

const preference = 'bridge.welcomeSeen.v1';
const unselected = { selected: null, localVm: 'idle', model: 'locked' };
const jobId = '12345678-1234-4abc-8def-123456789abc';
const requirements = { memoryBytes: 6442450944, stateDiskBytes: 34359738368, downloadBytes: 2800000000, setupDiskBytes: null, freeDiskBytes: null, readySeconds: null };
function native(initial: object = unselected, sizing: unknown = requirements) {
  vi.stubGlobal('isTauri', true);
  let status = initial;
  const ipc = vi.fn((command: string, args?: unknown) => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return status;
    if (command === 'setup_requirements') return sizing;
    if (command === 'first_job_begin') {
      const prompt = String(typeof args === 'object' && args !== null && !Array.isArray(args) && 'prompt' in args ? args.prompt : '');
      const existing = 'firstJob' in status ? status.firstJob as { prompt: string } : undefined;
      if (existing && existing.prompt !== prompt) throw Error('different job');
      const job = existing ?? { id: jobId, prompt, phase: 'setup' };
      status = { ...status, firstJob: job };
      return job;
    }
    if (command === 'first_job_discard') {
      status = { ...status };
      delete (status as { firstJob?: unknown }).firstJob;
      return;
    }
    if (command === 'setup_select_local') {
      status = { ...status, selected: 'localVm', localVm: 'starting', model: 'locked' };
      return status;
    }
    throw new Error('Unsupported command');
  });
  mockIPC(ipc);
  return ipc;
}
beforeEach(() => { localStorage.clear(); });
afterEach(() => { cleanup(); clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

test('job intake saves native intent before setup and restores it without the renderer preference', async () => {
  const ipc = native(); const user = userEvent.setup(); const first = render(<App />);
  expect(await screen.findByRole('heading', { name: 'What do you want done?' })).toBeTruthy();
  expect(screen.queryByRole('navigation', { name: 'Main' })).toBeNull();
  expect(screen.queryByRole('contentinfo')).toBeNull();
  const field = screen.getByRole('textbox', { name: 'What do you want done?' });
  const submit = screen.getByRole('button', { name: 'Save job and set up' });
  expect(submit.matches(':disabled')).toBe(true);
  await user.type(field, 'Summarize the release notes');
  await user.click(submit);
  const heading = await screen.findByRole('heading', { name: 'Getting your workspace ready' });
  expect(document.activeElement).toBe(heading);
  expect(screen.getByText('Summarize the release notes')).toBeTruthy();
  expect(localStorage.getItem(preference)).toBe('1');
  expect(ipc.mock.calls.some(([command]) => command === 'setup_select_local')).toBe(false);
  localStorage.clear(); first.unmount(); render(<App />);
  expect(await screen.findByRole('heading', { name: 'Getting your workspace ready' })).toBeTruthy();
  expect(screen.getByText('Summarize the release notes')).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'What do you want done?' })).toBeNull();
  expect(ipc.mock.calls.filter(([command]) => command === 'first_job_begin')).toHaveLength(1);
  expect(ipc.mock.calls.some(([command]) => command === 'setup_select_local')).toBe(false);
});

test('starter jobs fill and focus the box without saving or starting anything', async () => {
  const ipc = native(); const user = userEvent.setup(); render(<App />);
  await screen.findByRole('heading', { name: 'What do you want done?' });
  await user.click(screen.getByRole('button', { name: 'Compare two approaches' }));
  const field = screen.getByRole<HTMLTextAreaElement>('textbox', { name: 'What do you want done?' });
  expect(field.value).toBe('Compare two approaches');
  expect(document.activeElement).toBe(field);
  expect(screen.getByRole('button', { name: 'Save job and set up' }).matches(':disabled')).toBe(false);
  expect(ipc.mock.calls.some(([command]) => ['first_job_begin', 'setup_select_local'].includes(command))).toBe(false);
});

test('job setup keeps preflight authoritative and never arms the bundled first test', async () => {
  const ipc = native(); const user = userEvent.setup(); render(<App />);
  await user.type(await screen.findByRole('textbox', { name: 'What do you want done?' }), 'Summarize https://example.com');
  await user.click(screen.getByRole('button', { name: 'Save job and set up' }));
  expect(await screen.findByRole('group', { name: 'Local workspace' })).toBeTruthy();
  expect(screen.queryByRole('radio', { name: 'On a cluster you already have' })).toBeNull();
  expect(screen.getByRole('region', { name: 'Computer requirements' })).toBeTruthy();
  const next = screen.getByRole('button', { name: 'Continue setup' });
  expect(next.matches(':disabled')).toBe(false);
  expect(localStorage.getItem('bridge.firstTest.v1')).toBeNull();
  await user.click(next);
  expect(await screen.findByRole('list', { name: 'Setup steps' })).toBeTruthy();
  expect(screen.getAllByRole('listitem').map(row => row.querySelector('b')?.textContent)).toEqual(['Set up', 'Connect the AI', 'Run your job']);
  expect(within(screen.getByRole('navigation')).getByText('Setting up, step 1 of 3')).toBeTruthy();
  expect(localStorage.getItem('bridge.firstTest.v1')).toBeNull();
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_select_local')).toHaveLength(1);
  expect(ipc.mock.calls.some(([command]) => ['setup_example', 'agent_run'].includes(command))).toBe(false);
});

test('a failed native save keeps the typed job and starts no setup side effect', async () => {
  const ipc = native(); mockIPC((command, args) => command === 'first_job_begin' ? Promise.reject('PRIVATE_PATH') : ipc(command, args));
  const user = userEvent.setup(); render(<App />);
  const field = await screen.findByRole<HTMLTextAreaElement>('textbox', { name: 'What do you want done?' });
  await user.type(field, 'Keep this exact draft');
  await user.click(screen.getByRole('button', { name: 'Save job and set up' }));
  expect((await screen.findByRole('alert')).textContent).toBe('The job could not be saved. Check the text and try again.');
  expect(field.value).toBe('Keep this exact draft');
  expect(document.activeElement).toBe(field);
  expect(document.body.textContent).not.toContain('PRIVATE_PATH');
  expect(ipc.mock.calls.some(([command]) => command === 'setup_select_local')).toBe(false);
});

test('oversized UTF-8 input is explained before native save', async () => {
  const ipc = native(); const user = userEvent.setup(); render(<App />);
  const field = await screen.findByRole<HTMLTextAreaElement>('textbox', { name: 'What do you want done?' });
  fireEvent.change(field, { target: { value: '界'.repeat(3000) } });
  await user.click(screen.getByRole('button', { name: 'Save job and set up' }));
  expect((await screen.findByRole('alert')).textContent).toBe('This job is too long. Shorten it to 8,192 bytes or less.');
  expect(field.getAttribute('aria-invalid')).toBe('true');
  expect(document.activeElement).toBe(field);
  expect(ipc.mock.calls.some(([command]) => command === 'first_job_begin')).toBe(false);
});

test('discard removes only the saved job and returns to intake without changing the workspace', async () => {
  const firstJob = { id: jobId, prompt: 'A saved job', phase: 'setup' };
  const ipc = native({ ...unselected, firstJob }); const user = userEvent.setup(); render(<App />);
  expect(await screen.findByText('A saved job')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Discard job' }));
  expect(await screen.findByRole('heading', { name: 'What do you want done?' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Save job and set up' }).matches(':disabled')).toBe(true);
  expect(screen.queryByText('A saved job')).toBeNull();
  expect(ipc.mock.calls.filter(([command]) => command === 'first_job_discard')).toEqual([['first_job_discard', { id: jobId }]]);
  expect(ipc.mock.calls.some(([command]) => ['setup_select_local', 'setup_change_local'].includes(command))).toBe(false);
});

test('reopening local setup keeps the saved job steps instead of returning-workspace mode', async () => {
  const firstJob = { id: jobId, prompt: 'Keep this job visible', phase: 'setup' };
  native({ selected: 'localVm', localVm: 'ready', model: 'available', firstJob });
  const user = userEvent.setup(); render(<SetupHarness onOpenExample={() => {}} firstCheck="passed" />);
  expect(await screen.findByText('Keep this job visible')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  const local = await screen.findByRole('region', { name: 'This computer' });
  await user.click(within(local).getByRole('button', { name: 'Connect the AI' }));
  expect(await screen.findByRole('list', { name: 'Setup steps' })).toBeTruthy();
  expect(screen.getByText('Run your job', { selector: 'li b' })).toBeTruthy();
  expect(screen.queryByRole('region', { name: 'Workspace' })).toBeNull();
});

test('a late status reply cannot erase an acknowledged saved job', async () => {
  vi.stubGlobal('isTauri', true);
  let status: Record<string, unknown> = { ...unselected };
  let hold = false; let release!: () => void;
  const ipc = vi.fn((command: string, args?: unknown) => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_requirements') return requirements;
    if (command === 'setup_status') {
      if (!hold) return status;
      hold = false; const stale = { ...status };
      return new Promise(resolve => { release = () => resolve(stale); });
    }
    if (command === 'first_job_begin') {
      const prompt = String(typeof args === 'object' && args !== null && !Array.isArray(args) && 'prompt' in args ? args.prompt : '');
      const job = { id: jobId, prompt, phase: 'setup' };
      status = { ...status, firstJob: job }; return job;
    }
    throw Error(`Unexpected ${command}`);
  });
  mockIPC(ipc); const user = userEvent.setup(); render(<App />);
  const field = await screen.findByRole('textbox', { name: 'What do you want done?' });
  hold = true; act(() => window.dispatchEvent(new Event('focus')));
  await waitFor(() => expect(release).toBeTypeOf('function'));
  await user.type(field, 'Keep the saved job');
  await user.click(screen.getByRole('button', { name: 'Save job and set up' }));
  expect(await screen.findByText('Keep the saved job')).toBeTruthy();
  await act(async () => release());
  expect(screen.getByText('Keep the saved job')).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'What do you want done?' })).toBeNull();
  expect(localStorage.getItem('bridge.firstTest.v1')).toBeNull();
});

test('a late status reply cannot restore a discarded job', async () => {
  vi.stubGlobal('isTauri', true);
  const firstJob = { id: jobId, prompt: 'Discard this job', phase: 'setup' };
  let status: Record<string, unknown> = { ...unselected, firstJob };
  let hold = false; let release!: () => void;
  mockIPC((command: string) => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_requirements') return requirements;
    if (command === 'setup_status') {
      if (!hold) return status;
      hold = false; const stale = { ...status };
      return new Promise(resolve => { release = () => resolve(stale); });
    }
    if (command === 'first_job_discard') { status = { ...unselected }; return; }
    throw Error(`Unexpected ${command}`);
  });
  const user = userEvent.setup(); render(<App />);
  expect(await screen.findByText('Discard this job')).toBeTruthy();
  hold = true; act(() => window.dispatchEvent(new Event('focus')));
  await waitFor(() => expect(release).toBeTypeOf('function'));
  await user.click(screen.getByRole('button', { name: 'Discard job' }));
  expect(await screen.findByRole('heading', { name: 'What do you want done?' })).toBeTruthy();
  await act(async () => release());
  expect(screen.getByRole('heading', { name: 'What do you want done?' })).toBeTruthy();
  expect(screen.queryByText('Discard this job')).toBeNull();
});

test('a saved native choice skips welcome even without a renderer preference', async () => {
  const ipc = native({ selected: 'localVm', localVm: 'starting', model: 'locked' }); render(<App />);
  expect(await screen.findByText('Starting the workspace…')).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'What do you want done?' })).toBeNull();
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
  expect(screen.getByText(/workspace includes Orka/)).toBeTruthy();
  expect(ipc.mock.calls.some(([command]) => command === 'setup_select_local')).toBe(false);
  const next = screen.getByRole('button', { name: 'Continue' });
  next.focus(); await user.keyboard('{Enter}{Enter}');
  expect(await screen.findByText('Starting the workspace…')).toBeTruthy();
  expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'This computer' }));
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_select_local')).toEqual([['setup_select_local', {}]]);
});

test('requirements render the native build values rather than fixed memory or disk sizes', async () => {
  localStorage.setItem(preference, '1'); native(unselected, { memoryBytes: 8589934592, stateDiskBytes: 68719476736, downloadBytes: 2380070756, setupDiskBytes: null, freeDiskBytes: null, readySeconds: null });
  render(<App />); await openLocalSetup();
  expect(screen.getByText('Downloads about 2.4 GB. Needs virtualization and 8.6 GB of memory. Data disk can grow to 68.7 GB.')).toBeTruthy();
  expect(screen.getByText('Use Orka on a Kubernetes cluster you already have.')).toBeTruthy();
  expect(screen.getAllByText(/step 1 of 4/i)).toHaveLength(1);
  expect(within(screen.getByRole('contentinfo')).queryByText(/step 1/)).toBeNull();
  await userEvent.setup().click(screen.getByRole('button', { name: "What's inside" }));
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(false);
  expect(screen.queryByText(/GiB|free disk|after installation/)).toBeNull();
});

test('debug requirements without a download retain RAM checks and disk facts without inventing a transfer', async () => {
  localStorage.setItem(preference, '1'); native(unselected, { ...requirements, downloadBytes: null });
  render(<App />); await openLocalSetup();
  expect(await screen.findByText('Needs virtualization and 6.4 GB of memory. Data disk can grow to 34.4 GB.')).toBeTruthy();
  expect(screen.queryByText(/Downloads about/)).toBeNull();
  expect(screen.getByRole('region', { name: 'Computer requirements' })).toBeTruthy();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(false));
});

test('release welcome exposes unavailability before job submission', async () => {
  const ipc = native({ ...unselected, localVm: 'notIntegrated' }, null); render(<App />);
  expect(await screen.findByRole('button', { name: 'Save job and set up' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Save job and set up' }).matches(':disabled')).toBe(true);
  expect(screen.getByText('Workspace setup is not included in this build. Nothing will be downloaded or started.')).toBeTruthy();
  expect(screen.getByText('Run your job')).toBeTruthy();
  expect(ipc.mock.calls.some(([command]) => command === 'setup_select_local')).toBe(false);
});

test('release-unavailable setup stays explicit and does not invent requirements', async () => {
  localStorage.setItem(preference, '1'); native({ ...unselected, localVm: 'notIntegrated' }, null); render(<App />);
  expect(await screen.findByText('Workspace setup is not included in this build. Nothing will be downloaded or started.')).toBeTruthy();
  expect(screen.queryByText(/GiB/)).toBeNull();
});

test.each(['getItem', 'setItem'] as const)('denied preference %s does not prevent setup', async method => {
  native(); vi.spyOn(Storage.prototype, method).mockImplementation(() => { throw new DOMException('Denied', 'SecurityError'); });
  const user = userEvent.setup(); render(<App />);
  await user.type(await screen.findByRole('textbox', { name: 'What do you want done?' }), 'Explain the API');
  await user.click(screen.getByRole('button', { name: 'Save job and set up' }));
  expect(await screen.findByRole('heading', { name: 'Getting your workspace ready' })).toBeTruthy();
  expect(screen.getByText('Explain the API')).toBeTruthy();
});

test.each([
  { ...requirements, imageBytes: 3651994962 },
  { ...requirements, memoryBytes: Number.MAX_SAFE_INTEGER + 1 },
  { ...requirements, stateDiskBytes: 0 },
  { ...requirements, downloadBytes: 0 },
  { ...requirements, downloadBytes: 'SECRET_MARKER' },
  { memoryBytes: 'SECRET_MARKER', stateDiskBytes: 123 },
  { ...requirements, secret: 'SECRET_MARKER' },
  { ...requirements, readySeconds: -1 },
  { ...requirements, setupDiskBytes: 'SECRET_MARKER' },
  { memoryBytes: 6442450944, stateDiskBytes: 34359738368, downloadBytes: 2800000000 },
])('unsupported sizing cannot produce requirements copy or leak data: %j', async value => {
  localStorage.setItem(preference, '1'); native(unselected, value); render(<App />); await openLocalSetup();
  expect(await screen.findByText(/Requirements are not available yet for this build\./)).toBeTruthy();
  await userEvent.setup().click(screen.getByRole('button', { name: "What's inside" }));
  expect(document.body.textContent).not.toMatch(/\d[.,]\d GB|GiB/);
  expect(document.body.textContent).not.toContain('SECRET_MARKER');
});

test('an unseen welcome cannot hide a failed status read or overwrite its preference', async () => {
  native(); let failed = true;
  const ipc = vi.fn(command => command === 'workspace_upgrade_status' ? noUpgrade : command === 'setup_status'
    ? failed ? Promise.reject('SECRET_MARKER') : { selected: 'localVm', localVm: 'starting', model: 'locked' }
    : command === 'setup_requirements' ? requirements : Promise.reject({ kind: 'unavailable' }));
  mockIPC(ipc); const user = userEvent.setup(); render(<App />);
  expect(await screen.findByRole('alert')).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'What do you want done?' })).toBeNull();
  expect(localStorage.getItem(preference)).toBeNull();
  failed = false;
  await user.click(screen.getByRole('button', { name: 'Check again' }));
  expect(await screen.findByText('Starting the workspace…')).toBeTruthy();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(localStorage.getItem(preference)).toBeNull();
  expect(ipc.mock.calls.some(([command]) => command === 'setup_select_local')).toBe(false);
});

test('a delayed initial status cannot flash welcome over an existing workspace', async () => {
  native(); let complete!: (value: unknown) => void;
  mockIPC(command => command === 'workspace_upgrade_status' ? noUpgrade : command === 'setup_status' ? new Promise(resolve => { complete = resolve; })
    : command === 'setup_requirements' ? requirements : Promise.reject({ kind: 'unavailable' }));
  render(<App />);
  expect(screen.queryByRole('heading', { name: 'What do you want done?' })).toBeNull();
  complete({ selected: 'localVm', localVm: 'starting', model: 'locked' });
  await waitFor(() => expect(screen.getByText('Starting the workspace…')).toBeTruthy());
  expect(screen.queryByRole('heading', { name: 'What do you want done?' })).toBeNull();
});

const measuredMac = { memoryBytes: 6442450944, stateDiskBytes: 34359738368, downloadBytes: 2380070756, setupDiskBytes: 6997696512, freeDiskBytes: 24257590349, readySeconds: 134 };
test('a measured image shows its setup time on Welcome and its free and kept disk on the choice', async () => {
  native(unselected, measuredMac); const user = userEvent.setup(); render(<App />);
  expect(await screen.findByText(/Mostly a 2.4 GB download, then about 2 minutes to start/)).toBeTruthy();
  await user.type(screen.getByRole('textbox', { name: 'What do you want done?' }), 'Summarize a page');
  await user.click(screen.getByRole('button', { name: 'Save job and set up' }));
  expect(await screen.findByText('Downloads about 2.4 GB. Needs virtualization, 6.4 GB of memory and 24.3 GB of free disk; about 7.0 GB stays in use. Data disk can grow to 34.4 GB.')).toBeTruthy();
});

test('Welcome claims no time or size it was not given', async () => {
  native(unselected, null); render(<App />);
  expect(await screen.findByText(/Mostly a download/)).toBeTruthy();
  expect(document.body.textContent).not.toMatch(/minute|\d GB/);
});
