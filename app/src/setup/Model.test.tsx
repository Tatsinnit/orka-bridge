import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Model from './Model';
import userEvent from '@testing-library/user-event';
import { connectModel } from './modelState';

const { invoke, isTauri } = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri }));
beforeEach(() => { invoke.mockReset(); isTauri.mockReturnValue(true); });
afterEach(() => { cleanup(); });
test('remove is explicit, excludes duplicates and permits connecting again', async () => {
  const onState = vi.fn();
  invoke.mockResolvedValue({ ready: true });
  render(<Model enabled onState={onState} />);
  expect(invoke).not.toHaveBeenCalled();
  expect(screen.queryByRole('button', { name: 'Remove sign-in' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText(/^Connected$/);
  expect(screen.getByRole('status').textContent).toBe('Connected');
  expect(screen.queryByText(/Use GitHub Copilot with GitHub CLI/)).toBeNull();
  expect(screen.queryByText(/Your sign-in is shared only/)).toBeNull();
  let complete!: (value: unknown) => void;
  invoke.mockImplementation(() => new Promise(resolve => { complete = resolve; }));
  fireEvent.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  expect(screen.getByRole('group', { name: 'Remove sign-in confirmation' }).textContent).toContain('GitHub CLI stays signed in');
  expect(invoke.mock.calls.filter(([cmd]) => cmd === 'setup_remove_model')).toHaveLength(0);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(invoke.mock.calls.filter(([cmd]) => cmd === 'setup_remove_model')).toHaveLength(0);
  fireEvent.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  const confirm = screen.getByRole('button', { name: 'Remove sign-in' });
  fireEvent.click(confirm); fireEvent.click(confirm);
  expect(invoke.mock.calls.filter(([cmd]) => cmd === 'setup_remove_model')).toEqual([['setup_remove_model']]);
  expect(screen.queryByText(/^Connected$/)).toBeNull();
  await act(async () => { complete({ ready: false }); });
  expect(screen.getAllByText(/GitHub CLI stays signed in/).length).toBeGreaterThan(0);
  expect(screen.getByRole('button', { name: 'Connect the AI' }).hasAttribute('disabled')).toBe(false);
  expect(onState).toHaveBeenLastCalledWith('idle');
  expect(screen.queryByRole('button', { name: 'Remove sign-in' })).toBeNull();
});
test('uncertain connection failure retains explicit cleanup without echoing raw failures', async () => {
  invoke.mockRejectedValue({ kind: 'modelConnect', message: 'synthetic-private-token' });
  const view = render(<Model enabled />);
  expect(screen.queryByRole('button', { name: 'Remove sign-in' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByRole('alert');
  view.rerender(<Model enabled={false} />);
  expect(screen.getByRole('button', { name: 'Remove sign-in' }).hasAttribute('disabled')).toBe(true);
  view.rerender(<Model enabled />);
  invoke.mockRejectedValue({ kind: 'invented', message: 'synthetic-private-token' });
  fireEvent.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  fireEvent.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('could not confirm sign-in removal'));
  expect(screen.getByRole('alert').textContent).not.toContain('Remove sign-in again');
  expect(document.body.textContent).not.toContain('synthetic-private-token');
  expect(screen.queryByText(/no longer has a saved sign-in/)).toBeNull();
  view.rerender(<Model enabled paused />);
  expect(screen.getByRole('button', { name: 'Retry removal' }).hasAttribute('disabled')).toBe(true);
});
test('failed removal retries cleanup rather than Connect and copies its own operation', async () => {
  const user = userEvent.setup(); const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  invoke.mockRejectedValue({ kind: 'modelStore', native: { call: 'CredDeleteW', code: 5 }, message: 'SECRET' });
  invoke.mockResolvedValueOnce({ ready: true });
  render(<Model enabled />);
  await user.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText(/^Connected$/);
  await user.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  expect(invoke).toHaveBeenCalledExactlyOnceWith('setup_connect_model');
  await user.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Retry removal');
  expect(screen.queryByRole('button', { name: 'Connect the AI' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Retry removal' }).className).toContain('button button-primary');
  expect(screen.getByRole('button', { name: 'Retry removal' }).parentElement?.querySelector('button')?.textContent).toBe('Retry removal');
  await user.click(screen.getByRole('button', { name: 'Copy details for help' }));
  expect(copy.mock.calls[0][0]).toBe('Bridge AI connection\nOperation: Removing sign-in\nCategory: modelStore\nOS code: 5');
  invoke.mockResolvedValue({ ready: false });
  await user.click(screen.getByRole('button', { name: 'Retry removal' }));
  await screen.findByText(/no longer has a saved sign-in/);
  expect(screen.getByRole('button', { name: 'Connect the AI' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Remove sign-in' })).toBeNull();
  expect(screen.queryByText(/^Connected$/)).toBeNull();
  expect(invoke.mock.calls.filter(([cmd]) => cmd === 'setup_remove_model')).toHaveLength(2);
  expect(invoke.mock.calls.filter(([cmd]) => cmd === 'setup_connect_model')).toHaveLength(1);
  expect(document.body.textContent).not.toContain('SECRET');
});
test.each(['paused', 'locked'])('removal confirmation cannot bypass the %s credential gate', async gate => {
  invoke.mockResolvedValue({ ready: false });
  invoke.mockResolvedValueOnce({ ready: true });
  const view = render(<Model enabled />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText(/^Connected$/);
  fireEvent.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  view.rerender(<Model enabled={gate !== 'locked'} paused={gate === 'paused'} />);
  fireEvent.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  expect(invoke).toHaveBeenCalledExactlyOnceWith('setup_connect_model');
  // Cancel remains harmless even while the credential operation is blocked.
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(invoke).toHaveBeenCalledExactlyOnceWith('setup_connect_model');
});
test('CLI recovery puts retry before copy and quiet help in one action row', async () => {
  invoke.mockRejectedValue({ kind: 'modelCaptureUnavailable' });
  render(<Model enabled />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByRole('alert');
  const retry = screen.getByRole('button', { name: 'Check again' });
  const row = retry.closest('.workspace-step-actions')!;
  const buttons = [...row.querySelectorAll('button')];
  expect(buttons.map(button => button.textContent)).toEqual(['Check again', 'Copy details for help', 'Install GitHub CLI', 'Sign-in help']);
  expect(row.querySelectorAll('.button-primary')).toHaveLength(1);
  expect(row.querySelectorAll('.button-secondary')).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Install GitHub CLI' }).classList.contains('button-quiet')).toBe(true);
  expect(screen.getByRole('button', { name: 'Sign-in help' }).classList.contains('button-quiet')).toBe(true);
});

test('locked model cannot invoke credentials', () => {
  render(<Model enabled={false} />);
  expect(screen.getByRole('button', { name: 'Connect the AI' }).hasAttribute('disabled')).toBe(true);
  expect(invoke).not.toHaveBeenCalled();
});
test('explicit sign-in uses no renderer arguments, excludes duplicate clicks and shows acknowledgment', async () => {
  let complete!: (value: unknown) => void;
  invoke.mockImplementation(() => new Promise(resolve => { complete = resolve; }));
  render(<Model enabled />);
  const button = screen.getByRole('button', { name: 'Connect the AI' });
  fireEvent.click(button); fireEvent.click(button);
  expect(invoke).toHaveBeenCalledExactlyOnceWith('setup_connect_model');
  // Busy is status text, not a disabled action button.
  expect(screen.queryByRole('button', { name: /Connecting model/ })).toBeNull();
  expect(screen.getByText('Connecting the AI…').closest('button')).toBeNull();
  await act(async () => { complete({ ready: true }); });
  expect(screen.getByText('Connected')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Model connected' })).toBeNull();
  expect(screen.getByText('Connected').closest('button')).toBeNull();
  expect(screen.queryByText(/inference/)).toBeNull();
  expect(screen.queryByText(/run the example/)).toBeNull();
});
test('known native stage error is shown without raw message; unknown errors never echo', async () => {
  invoke.mockRejectedValue({ kind: 'modelConnect', message: 'synthetic-private-token /path/raw-stderr' });
  render(<Model enabled />); fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('Bridge could not confirm the AI connection'));
  expect(screen.queryByText(/synthetic-private-token/)).toBeNull();
  invoke.mockRejectedValue({ kind: 'invented', message: 'synthetic-private-token' });
  const result = await connectModel();
  expect(result).toMatchObject({ ok: false, message: 'The AI connection could not be completed.' });
});
test('a full Windows credential store explains sign-in failure without exposing the native message', async () => {
  invoke.mockRejectedValue({ kind: 'modelStore', message: 'synthetic-private-token /path/raw-stderr',
    native: { call: 'CredWriteW', code: 8 } });
  render(<Model enabled />); fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect((await screen.findByRole('alert')).textContent).toContain(
    "Windows couldn't save the sign-in. Windows Credential Manager is full (CredWriteW, error 8).");
  expect(document.body.textContent).not.toContain('synthetic-private-token');
});
test('native store details accept only known calls and unsigned Windows error codes', async () => {
  invoke.mockRejectedValue({ kind: 'modelStore', native: { call: 'CredReadW', code: 1168 } });
  expect(await connectModel()).toMatchObject({ ok: false,
    message: "Windows couldn't load the sign-in (CredReadW, error 1168)." });
  for (const native of [null, { call: 'synthetic-private-token', code: 8 },
    { call: 'CredWriteW', code: '8' }, { call: 'CredWriteW', code: -1 },
    { call: 'CredWriteW', code: 1.5 }, { call: 'CredWriteW', code: 4294967296 }]) {
    invoke.mockRejectedValue({ kind: 'modelStore', message: 'synthetic-private-token', native });
    expect(await connectModel()).toMatchObject({ ok: false,
      message: 'Bridge could not save or load your sign-in.' });
  }
});
test('late success after runtime closes does not display ready', async () => {
  let complete!: (value: unknown) => void;
  invoke.mockImplementation(() => new Promise(resolve => { complete = resolve; }));
  const view = render(<Model enabled />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  view.rerender(<Model enabled={false} />);
  await act(async () => { complete({ ready: true }); });
  expect(screen.queryByText(/^Connected$/)).toBeNull();
});
test('failed CLI recovery offers install and sign-in help and explicitly reconnects', async () => {
  invoke.mockRejectedValue({ kind: 'modelCaptureUnavailable', message: 'SECRET' });
  const observe = vi.fn(); render(<Model enabled onState={observe} />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('gh auth login --hostname github.com --web');
  expect(alert.textContent).toContain('local cluster and your agent files are kept');
  expect(screen.getByRole('button', { name: 'Install GitHub CLI' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Sign-in help' })).toBeTruthy();
  expect(alert.textContent).not.toContain('SECRET');
  await waitFor(() => expect(observe).toHaveBeenLastCalledWith('failed'));
  invoke.mockResolvedValue({ ready: true });
  const retry = screen.getByRole('button', { name: 'Check again' });
  fireEvent.click(retry); fireEvent.click(retry);
  await screen.findByText(/^Connected$/);
  expect(invoke.mock.calls.filter(([command]) => command === 'setup_connect_model')).toHaveLength(2);
});
test('reading sign-in errors show the status command without copying diagnostics', async () => {
  invoke.mockRejectedValue({ kind: 'modelCaptureFailed', message: 'SECRET', diagnostic: 'SECRET' });
  render(<Model enabled />); fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect((await screen.findByRole('alert')).textContent).toContain('gh auth status --hostname github.com');
  expect(screen.getByRole('button', { name: 'Copy details for help' })).toBeTruthy();
  expect(document.body.textContent).not.toContain('SECRET');
});

test('keyboard connection moves focus to its busy status and back to the retry action', async () => {
  let reject!: (error: unknown) => void;
  invoke.mockImplementation(() => new Promise((_resolve, fail) => { reject = fail; }));
  const user = userEvent.setup(); render(<Model enabled />);
  const button = screen.getByRole('button', { name: 'Connect the AI' });
  button.focus(); await user.keyboard('{Enter}');
  expect(screen.queryByRole('button', { name: /Connecting/ })).toBeNull();
  expect(document.activeElement).toBe(screen.getByText('Connecting the AI…').closest('[tabindex="-1"]'));
  await act(async () => reject({ kind: 'modelConnect' }));
  const retry = await screen.findByRole('button', { name: 'Check again' });
  expect(document.activeElement).toBe(retry);
  await user.keyboard('{Enter}');
  expect(screen.getByText('Connecting the AI…')).toBeTruthy();
  await act(async () => reject({ kind: 'modelConnect' }));
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Check again' }));
});

test('help buttons select native topics and copying cannot include arbitrary native data', async () => {
  invoke.mockRejectedValue({ kind: 'modelCaptureUnavailable', message: 'SECRET /private/path', diagnostic: 'code: 5' });
  const user = userEvent.setup(); const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  render(<Model enabled />); await user.click(screen.getByRole('button', { name: 'Connect the AI' }));
  invoke.mockResolvedValue(null);
  await user.click(await screen.findByRole('button', { name: 'Install GitHub CLI' }));
  expect(invoke).toHaveBeenLastCalledWith('setup_open_help', { topic: 'githubCliInstall' });
  await user.click(screen.getByRole('button', { name: 'Sign-in help' }));
  expect(invoke).toHaveBeenLastCalledWith('setup_open_help', { topic: 'githubSignIn' });
  await user.click(screen.getByRole('button', { name: 'Copy details for help' }));
  expect(copy.mock.calls[0][0]).toBe('Bridge AI connection\nOperation: Connecting AI\nCategory: modelCaptureUnavailable');
  invoke.mockRejectedValue('SECRET'); copy.mockRejectedValue('SECRET');
  await user.click(screen.getByRole('button', { name: 'Copy details for help' }));
  expect(screen.getByRole('status').textContent).toContain("Couldn't copy. Share the visible error message for help.");
  await user.click(screen.getByRole('button', { name: 'Sign-in help' }));
  expect(screen.getByRole('status').textContent).toContain("Couldn't open help");
  expect(document.body.textContent).not.toContain('SECRET');
});

test('preview and malformed responses cannot claim ready', async () => {
  isTauri.mockReturnValue(false);
  expect((await connectModel()).ok).toBe(false); expect(invoke).not.toHaveBeenCalled();
  isTauri.mockReturnValue(true);
  for (const response of [{ ready: false }, { ready: true, token: 'synthetic-private-token' }, null]) {
    invoke.mockResolvedValue(response); expect((await connectModel()).ok).toBe(false);
  }
});

// Queued intent: in-memory consent to connect once Orka is ready. Credentials are untouched before then.
test('a choice while the workspace is starting is queued without any credential command', () => {
  const onState = vi.fn();
  render(<Model enabled={false} queueable onState={onState} />);
  expect(screen.getByRole('heading', { name: 'Connect the AI' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Remove sign-in' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect(invoke).not.toHaveBeenCalled();
  expect(screen.getByText('Queued').closest('button')).toBeNull();
  expect(screen.getByText(/connects when the local cluster is ready/i)).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Connect the AI' })).toBeNull();
  expect(onState).toHaveBeenLastCalledWith('queued');
});

test('queued intent connects exactly once when the model becomes available, without another click', async () => {
  invoke.mockResolvedValue({ ready: true });
  const view = render(<Model enabled={false} queueable />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  view.rerender(<Model enabled paused={false} />);
  await screen.findByText(/^Connected$/);
  expect(invoke.mock.calls.filter(([cmd]) => cmd === 'setup_connect_model')).toHaveLength(1);
  view.rerender(<Model enabled={false} />); view.rerender(<Model enabled />);
  expect(invoke.mock.calls.filter(([cmd]) => cmd === 'setup_connect_model')).toHaveLength(1);
});

test('queued intent survives the readiness transition and fires only once unpaused and enabled', async () => {
  invoke.mockResolvedValue({ ready: true });
  const view = render(<Model enabled={false} queueable />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  // Ready but locked (not yet available): hold, neither fire nor clear.
  view.rerender(<Model enabled={false} queueable={false} />);
  expect(invoke).not.toHaveBeenCalled();
  expect(screen.getByText('Queued')).toBeTruthy();
  // Available but an unreadable observation or mutation pauses: hold.
  view.rerender(<Model enabled paused />);
  expect(invoke).not.toHaveBeenCalled();
  view.rerender(<Model enabled paused={false} />);
  await screen.findByText(/^Connected$/);
  expect(invoke).toHaveBeenCalledExactlyOnceWith('setup_connect_model');
});

test('a locked model that is not queueable offers no queue', () => {
  render(<Model enabled={false} />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect(screen.queryByText('Queued')).toBeNull();
  expect(invoke).not.toHaveBeenCalled();
});

test('cluster selection and cancelling clear intent so readiness needs fresh consent', () => {
  const view = render(<Model enabled={false} queueable />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  view.rerender(<Model enabled={false} queueable away />);
  view.rerender(<Model enabled queueable={false} away={false} />);
  expect(invoke).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: 'Connect the AI' })).toBeTruthy();
  view.rerender(<Model enabled={false} queueable />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  fireEvent.click(screen.getByRole('button', { name: 'Cancel queued connection' }));
  expect(screen.queryByText('Queued')).toBeNull();
  view.rerender(<Model enabled />);
  expect(invoke).not.toHaveBeenCalled();
});

test('a failed queued connection is not retried automatically', async () => {
  invoke.mockRejectedValue({ kind: 'modelConnect' });
  const view = render(<Model enabled={false} queueable />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  view.rerender(<Model enabled />);
  await screen.findByRole('alert');
  view.rerender(<Model enabled paused />); view.rerender(<Model enabled />);
  await act(async () => { await Promise.resolve(); });
  expect(invoke.mock.calls.filter(([cmd]) => cmd === 'setup_connect_model')).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Check again' })).toBeTruthy();
});

test('keyboard removal focuses Cancel, its operation, primary retry and then Connect without redundant confirmation', async () => {
  let settle!: (value: { ok: boolean }) => void;
  invoke.mockImplementation(() => new Promise((resolve, reject) => { settle = ({ ok }) => ok ? resolve({ ready: false }) : reject({ kind: 'modelStore' }); }));
  invoke.mockResolvedValueOnce({ ready: true });
  const user = userEvent.setup(); render(<Model enabled />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText(/^Connected$/);
  const remove = screen.getByRole('button', { name: 'Remove sign-in' });
  remove.focus(); await user.keyboard('{Enter}');
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Cancel' }));
  await user.keyboard('{Enter}');
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Remove sign-in' }));
  expect(invoke).toHaveBeenCalledExactlyOnceWith('setup_connect_model');
  await user.keyboard('{Enter}');
  await user.tab(); await user.keyboard('{Enter}');
  expect(screen.queryByRole('button', { name: 'Remove sign-in' })).toBeNull();
  expect(document.activeElement).not.toBe(document.body);
  expect(document.activeElement).toBe(screen.getByText('Removing sign-in…').closest('[tabindex="-1"]'));
  await act(async () => settle({ ok: false }));
  await screen.findByRole('alert');
  // Failed removal keeps the confirmed cleanup as the primary action, not Connect.
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Retry removal' }));
  expect(screen.queryByRole('button', { name: 'Connect the AI' })).toBeNull();
  await user.keyboard('{Enter}');
  expect(invoke.mock.calls.filter(([cmd]) => cmd === 'setup_remove_model')).toHaveLength(2);
  expect(document.activeElement).not.toBe(document.body);
  await act(async () => settle({ ok: true }));
  await screen.findByText(/no longer has a saved sign-in/);
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Connect the AI' }));
});

test('a mouse-started removal does not steal keyboard focus', async () => {
  invoke.mockResolvedValue({ ready: false });
  invoke.mockResolvedValueOnce({ ready: true });
  render(<Model enabled />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText(/^Connected$/);
  fireEvent.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  fireEvent.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  await screen.findByText(/no longer has a saved sign-in/);
  expect(document.activeElement).toBe(document.body);
});

// Focus follows an operation only while the creator has not moved it. These tests put another control in the page.
function withNeighbour(props: { enabled: boolean; queueable?: boolean }, hidden = false) {
  return <><div hidden={hidden}><Model {...props} /></div><button type="button">Elsewhere</button></>;
}
const neighbour = () => screen.getByRole('button', { name: 'Elsewhere' });

test('a queued keyboard choice does not take focus back when Orka becomes ready after the creator moved on', async () => {
  invoke.mockImplementation(() => new Promise(() => {}));
  const user = userEvent.setup(); const view = render(withNeighbour({ enabled: false, queueable: true }));
  screen.getByRole('button', { name: 'Connect the AI' }).focus(); await user.keyboard('{Enter}');
  expect(document.activeElement).toBe(screen.getByText('Queued').closest('[tabindex="-1"]'));
  await user.tab(); await user.tab();
  neighbour().focus();
  view.rerender(withNeighbour({ enabled: true, queueable: true }));
  await screen.findByText('Connecting the AI…');
  expect(document.activeElement).toBe(neighbour());
});

test('a settled connection does not pull focus from another control, on success or failure', async () => {
  let settle!: (ok: boolean) => void;
  invoke.mockImplementation(() => new Promise((resolve, reject) => { settle = ok => ok ? resolve({ ready: true }) : reject({ kind: 'modelConnect' }); }));
  const user = userEvent.setup(); render(withNeighbour({ enabled: true }));
  screen.getByRole('button', { name: 'Connect the AI' }).focus(); await user.keyboard('{Enter}');
  neighbour().focus();
  await act(async () => settle(false));
  await screen.findByRole('alert');
  expect(document.activeElement).toBe(neighbour());
  // The failed attempt is over; a later keyboard retry owns focus again.
  const retry = screen.getByRole('button', { name: 'Check again' });
  retry.focus(); await user.keyboard('{Enter}');
  neighbour().focus();
  await act(async () => settle(true));
  await screen.findByText(/^Connected$/);
  expect(document.activeElement).toBe(neighbour());
});

test('a pending removal does not pull focus from another control once it settles', async () => {
  let settle!: (ok: boolean) => void;
  invoke.mockImplementation(() => new Promise((resolve, reject) => { settle = ok => ok ? resolve({ ready: false }) : reject({ kind: 'modelStore' }); }));
  invoke.mockResolvedValueOnce({ ready: true });
  const user = userEvent.setup(); render(withNeighbour({ enabled: true }));
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText(/^Connected$/);
  screen.getByRole('button', { name: 'Remove sign-in' }).focus(); await user.keyboard('{Enter}');
  await user.tab(); await user.keyboard('{Enter}');
  neighbour().focus();
  await act(async () => settle(false));
  await screen.findByRole('alert');
  expect(document.activeElement).toBe(neighbour());
  screen.getByRole('button', { name: 'Retry removal' }).focus(); await user.keyboard('{Enter}');
  neighbour().focus();
  await act(async () => settle(true));
  await screen.findByText(/no longer has a saved sign-in/);
  expect(document.activeElement).toBe(neighbour());
});

test('a hidden Model never takes focus when its queued connection starts', async () => {
  invoke.mockImplementation(() => new Promise(() => {}));
  const user = userEvent.setup(); const view = render(withNeighbour({ enabled: false, queueable: true }));
  screen.getByRole('button', { name: 'Connect the AI' }).focus(); await user.keyboard('{Enter}');
  const status = screen.getByText('Queued').closest<HTMLElement>('[tabindex="-1"]')!;
  const focus = vi.spyOn(status, 'focus');
  // jsdom leaves focus on an element that a browser would drop when its page is hidden, so the Model must check visibility itself.
  view.rerender(withNeighbour({ enabled: false, queueable: true }, true));
  view.rerender(withNeighbour({ enabled: true, queueable: true }, true));
  await screen.findByText('Connecting the AI…');
  expect(focus).not.toHaveBeenCalled();
});
