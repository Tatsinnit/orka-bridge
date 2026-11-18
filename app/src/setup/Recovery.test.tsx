import { openLocalSetup } from './test-navigation';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../App';
import type { WorkspaceFailure } from './recovery-info';
import Recovery from './Recovery';

function native(failure?: WorkspaceFailure, raw = 'PRIVATE native diagnostic') {
  vi.stubGlobal('isTauri', true);
  let localVm = 'failed';
  const status = () => ({ selected: 'localVm', localVm, model: localVm === 'ready' ? 'available' : 'locked', localVmError: raw,
    ...(localVm === 'failed' && failure ? { workspaceFailure: failure } : {}) });
  const ipc = vi.fn((command: string) => {
    if (command === 'setup_status') return status();
    if (command === 'setup_select_local') { localVm = 'starting'; return status(); }
    if (command === 'setup_open_help') return null;
    if (command === 'setup_connect_model') throw { kind: 'modelConnect' };
    if (command === 'setup_example') return { agent: '---\nname: web-reader\n---\nRead a page.', tests: 'A check.' };
    throw Error('Unsupported fake command');
  });
  mockIPC(ipc);
  return { ipc, ready: () => { localVm = 'ready'; } };
}
beforeEach(() => localStorage.setItem('bridge.welcomeSeen.v1', '1'));
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

test.each([
  ['permission', 'does not have permission', 'workspace'],
  ['noSpace', "isn't enough free space", 'workspace'],
  ['integrity', "couldn't be verified", 'workspace'],
  ['inUse', 'still in use', 'workspace'],
  ['network', "couldn't be downloaded", undefined],
  ['preparation', "couldn't be prepared", undefined],
  ['guestConnection', "didn't finish starting", undefined],
  ['services', "services couldn't start", undefined],
  ['unknown', "Your workspace didn't start. Your agents are safe.", undefined],
] as const)('finite %s cause offers recovery and a way out', async (cause, summary, topic) => {
  const { ipc } = native({ cause, operation: cause === 'network' ? 'download' : 'start', codes: [] });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain(summary);
  if (topic) {
    await user.click(within(alert).getByRole('button', { name: 'Show me how' }));
    expect(ipc).toHaveBeenCalledWith('setup_open_help', { topic });
  }
  expect(screen.getByRole('contentinfo').textContent).toContain(summary);
  expect(screen.getByRole('button', { name: 'Use a cluster you already have instead' }).matches(':disabled')).toBe(false);
  expect(document.body.textContent).not.toContain('PRIVATE');
  await user.click(within(alert).getByRole('button', { name: 'Show details' }));
  expect(screen.getByText('PRIVATE native diagnostic').tagName).toBe('PRE');
});

test('download heading and preservation do not promise rejected bytes are kept', async () => {
  native({ cause: 'network', operation: 'download', codes: [] }); render(<App />); await openLocalSetup();
  expect(await screen.findByRole('heading', { level: 1, name: 'This computer' })).toBeTruthy();
  expect(screen.getByText("The workspace couldn't be downloaded")).toBeTruthy();
  expect(screen.getByRole('alert').textContent).toContain('Your agent files are kept. Try again to continue setup.');
});

test('keyboard retry calls native selection once and returns to progress', async () => {
  const { ipc } = native({ cause: 'unknown', operation: 'start', codes: [] });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  const retry = await screen.findByRole('button', { name: 'Try again' });
  retry.focus(); await user.keyboard('{Enter}{Enter}');
  await screen.findByRole('progressbar', { name: 'Workspace setup progress' });
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_select_local')).toHaveLength(1);
  expect(screen.queryByRole('alert')).toBeNull();
});

test('copy includes only native finite metadata, ignoring misleading raw diagnostics', async () => {
  native({ cause: 'permission', operation: 'start', codes: [5] }, 'Image(NoSpace) code: 4294967296 Bearer PRIVATE C:\\Users\\private');
  const user = userEvent.setup(); const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Copy details for help' }));
  expect(copy).toHaveBeenCalledTimes(1);
  expect(copy.mock.calls[0][0]).toContain('Category: permission');
  expect(copy.mock.calls[0][0]).toContain('OS code: 5');
  expect(copy.mock.calls[0][0]).not.toMatch(/NoSpace|4294967296|Bearer|PRIVATE|private|Users/);
});

test('raw text can neither classify the error nor supply copied OS codes', async () => {
  native(undefined, 'virtualizationDisabled NoSpace code: 5 startupStopFailed');
  const user = userEvent.setup(); const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(); render(<App />); await openLocalSetup();
  expect((await screen.findByRole('alert')).textContent).toContain("Your workspace didn't start. Your agents are safe.");
  await user.click(screen.getByRole('button', { name: 'Copy details for help' }));
  expect(copy.mock.calls[0][0]).not.toContain('code: 5');
});

test('copy and help failures do not echo rejected text', async () => {
  const { ipc } = native({ cause: 'noSpace', operation: 'prepare', codes: [] });
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation(command => { if (command === 'setup_open_help') throw 'SECRET'; return original(command); });
  const user = userEvent.setup(); vi.spyOn(navigator.clipboard, 'writeText').mockRejectedValue('SECRET'); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Copy details for help' }));
  expect(screen.getByText("Couldn't copy. Open Show details to read the error.")).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Show me how' }));
  expect(screen.getByText("Couldn't open help. Open Show details and ask for help with this error.")).toBeTruthy();
  expect(screen.queryByText('SECRET')).toBeNull();
});

test.each(['insufficientMemory', 'virtualizationDisabled', 'hypervisorUnavailable', 'virtualizationUnsupported'] as const)('copied preflight category retains %s without raw details', async preflightFailure => {
  const user = userEvent.setup(); const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  render(<Recovery preflightFailure={preflightFailure} detail="SECRET" disabled={false} onRetry={() => {}} />);
  await user.click(screen.getByRole('button', { name: 'Copy details for help' }));
  expect(copy.mock.calls[0][0]).toContain(`Category: ${preflightFailure}`);
  expect(copy.mock.calls[0][0]).not.toContain('SECRET');
});

test('typed unconfirmed stop cannot admit a second workspace', async () => {
  native({ cause: 'stopUnconfirmed', operation: 'stop', codes: [] }); render(<App />); await openLocalSetup();
  expect((await screen.findByRole('alert')).textContent).toContain("couldn't be stopped safely");
  expect(screen.getByRole('button', { name: 'Try again' }).matches(':disabled')).toBe(true);
  expect(screen.getByRole('button', { name: 'Try again' }).title).toBe('Quit Bridge and reopen it before trying again');
});

test.each(['workspaceUpdateRequired', 'workspaceVersionUnsupported'] as const)('version refusal %s cannot retry or replace the workspace', async cause => {
  const { ipc } = native({ cause, operation: 'verify', codes: [] });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('Open the matching Bridge version.');
  expect(alert.textContent).not.toContain("download couldn't be verified");
  const retry = within(alert).getByRole('button', { name: 'Try again' });
  expect(retry.matches(':disabled')).toBe(true);
  expect(retry.title).toBe('Open the matching Bridge version.');
  await user.click(retry);
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_select_local')).toHaveLength(0);
  expect(screen.queryByRole('button', { name: 'Set up new workspace' })).toBeNull();
});

test('sign-in removal stays in progress in the setup step and shell', async () => {
  const { ipc, ready } = native(); ready();
  const original = ipc.getMockImplementation()!;
  mockIPC(command => command === 'setup_remove_model' ? new Promise(() => {}) : original(command));
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  // A failed Connect may have saved a sign-in, so cleanup remains available without claiming connection.
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await screen.findByRole('alert');
  await user.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  await user.click(within(screen.getByRole('group', { name: 'Remove sign-in confirmation' })).getByRole('button', { name: 'Remove sign-in' }));
  const step = within(screen.getByRole('list', { name: 'Setup steps' })).getByText('Connect the AI', { selector: 'li b' }).closest('li')!;
  expect(step.textContent).toContain('Removing sign-in');
  expect(step.querySelector('.status-running')).toBeTruthy();
  expect(screen.getByRole('contentinfo').textContent).toContain('Removing the AI sign-in');
});

test('AI failure marks its setup step and every shell status without losing the running workspace', async () => {
  const { ready } = native(); ready();
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('Bridge could not confirm the AI connection'));
  expect(screen.getByText("Couldn't start").closest('li')?.className).toBe('step-failed');
  expect(within(screen.getByRole('navigation')).getByText('Setting up, step 3 of 4')).toBeTruthy();
  expect(screen.getByRole('contentinfo').textContent).toContain('AI not connected');
  expect(document.querySelector('.host-chip')?.getAttribute('title')).toBe('The workspace is ready on this computer.');
  expect(document.querySelector('.host-chip .live-dot')?.className).toContain('ready');
  expect(screen.getByRole('alert').textContent).toContain('Your workspace and agent files are kept');
});
