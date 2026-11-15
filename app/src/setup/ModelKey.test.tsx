import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Model from './Model';
import { connectModel } from './modelState';
const { invoke, isTauri } = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri }));
beforeEach(() => { invoke.mockReset(); isTauri.mockReturnValue(true); });
afterEach(cleanup);
const field = () => screen.getByLabelText('API key') as HTMLInputElement;
function keyChoice() { fireEvent.click(screen.getByRole('radio', { name: /Your own API key/ })); }
const key = 'sk-ant-synthetic-only-not-a-real-key';
const anthropic = { kind: 'key', vendor: 'anthropic', model: 'claude-sonnet-5-5' };

test('key stays only in its password field and is cleared as soon as native takes custody', async () => {
  let finish!: (value: unknown) => void;
  invoke.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  render(<Model enabled />); keyChoice();
  fireEvent.click(screen.getByRole('button', { name: 'Anthropic' }));
  expect(field().type).toBe('password'); expect(field().autocomplete).toBe('off'); expect(field().getAttribute('spellcheck')).toBe('false');
  expect(screen.getByText(/Never written to your agent files/)).toBeTruthy();
  expect(screen.queryByText(/Never written to a file/)).toBeNull();
  fireEvent.change(field(), { target: { value: key } });
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect(invoke).toHaveBeenCalledExactlyOnceWith('setup_connect_key', { vendor: 'anthropic', key });
  expect(field().value).toBe(''); expect(field().disabled).toBe(true);
  expect(screen.getByText('Checking')).toBeTruthy();
  expect(document.body.textContent).not.toContain(key);
  expect(JSON.stringify(localStorage)).not.toContain(key);
  await act(async () => finish({ ready: true, connection: anthropic }));
  expect(screen.getByText(/Anthropic with your API key/)).toBeTruthy();
  const change = screen.getByRole('button', { name: 'Change' });
  const remove = screen.getByRole('button', { name: 'Remove key' });
  const row = change.closest('.workspace-step-actions')!;
  expect([...row.querySelectorAll('button')].map(button => button.textContent)).toEqual(['Change', 'Remove key']);
  expect(change.classList.contains('button-secondary')).toBe(true);
  expect(remove.classList.contains('button-quiet')).toBe(true);
});
test('retry without a pasted key requests only a saved key, never implicit Copilot capture', async () => {
  invoke.mockRejectedValue({ kind: 'modelConnect' });
  render(<Model enabled />); keyChoice(); fireEvent.change(field(), { target: { value: key } });
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' })); await screen.findByRole('alert');
  fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
  expect(invoke.mock.calls[1]).toEqual(['setup_connect_model', { vendor: 'anthropic' }]);
});
test('a busy first key attempt restores selectors and requires a newly pasted key', async () => {
  invoke.mockRejectedValueOnce({ kind: 'modelBusy' }).mockResolvedValue({ ready: true, connection: anthropic });
  render(<Model enabled />); keyChoice(); fireEvent.change(field(), { target: { value: key } });
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' })); await screen.findByRole('alert');
  expect(screen.getByRole('radio', { name: 'GitHub Copilot' }).hasAttribute('disabled')).toBe(false);
  expect(screen.getByRole('button', { name: 'OpenAI' }).hasAttribute('disabled')).toBe(false);
  fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
  expect(invoke).toHaveBeenCalledTimes(1);
  fireEvent.change(field(), { target: { value: key } }); fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
  await screen.findByRole('button', { name: 'Change' });
  expect(invoke.mock.calls[1]).toEqual(['setup_connect_key', { vendor: 'anthropic', key }]);
});
test('malformed replacement after Change keeps the field and Connect action available', async () => {
  invoke.mockResolvedValueOnce({ ready: true, connection: anthropic }).mockResolvedValue({ ready: false });
  render(<Model enabled />); fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByRole('button', { name: 'Change' }); fireEvent.click(screen.getByRole('button', { name: 'Change' }));
  await screen.findByLabelText('API key'); fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect(field()).toBeTruthy(); expect(screen.getByRole('button', { name: 'Check again' })).toBeTruthy();
});
test.each(['modelCredentialRejected', 'modelProviderUnreachable', 'modelModelUnavailable'])('a Copilot %s error describes its retained sign-in, not an API key', async kind => {
  invoke.mockRejectedValue({ kind }); render(<Model enabled />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' })); await screen.findByRole('alert');
  expect(screen.getByRole('button', { name: 'Remove sign-in' })).toBeTruthy();
  expect(screen.queryByText(/no longer has a saved/)).toBeNull();
  expect(screen.getByRole('alert').textContent).not.toContain('this key');
});
test.each(['modelCaptureUnavailable', 'modelCaptureFailed', 'modelCaptureInvalid', 'modelCaptureTimedOut'])('Copilot %s before storage leaves the key choice available', async kind => {
  invoke.mockRejectedValueOnce({ kind }).mockResolvedValueOnce({ ready: true, connection: anthropic });
  render(<Model enabled />); fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' })); await screen.findByRole('alert');
  expect(screen.getByRole('radio', { name: /Your own API key/ }).matches(':disabled')).toBe(false);
  expect(screen.queryByRole('button', { name: 'Remove sign-in' })).toBeNull();
  keyChoice(); fireEvent.change(field(), { target: { value: key } }); fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByRole('button', { name: 'Change' });
  expect(invoke.mock.calls[1]).toEqual(['setup_connect_key', { vendor: 'anthropic', key }]);
});
test('empty or wrong-company key never crosses IPC; company changes clear the field', () => {
  render(<Model enabled />); keyChoice();
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect(screen.getByRole('alert').textContent).toContain('Paste the whole key'); expect(invoke).not.toHaveBeenCalled();
  fireEvent.change(field(), { target: { value: key } });
  fireEvent.click(screen.getByRole('button', { name: 'OpenAI' }));
  expect(field().value).toBe('');
  fireEvent.change(field(), { target: { value: key } });
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect(invoke).not.toHaveBeenCalled();
});
test('key cannot be queued or retained when a mounted card is hidden or the cluster changes', async () => {
  const view = render(<div><Model enabled={false} queueable /></div>); keyChoice();
  expect(field().disabled).toBe(true);
  expect(screen.getByRole('button', { name: 'Connect the AI' }).hasAttribute('disabled')).toBe(true);
  view.rerender(<div><Model enabled /></div>);
  fireEvent.change(field(), { target: { value: key } });
  view.rerender(<div hidden><Model enabled /></div>);
  await waitFor(() => expect((view.container.querySelector('input[type="password"]') as HTMLInputElement).value).toBe(''));
  view.rerender(<div><Model enabled /></div>);
  fireEvent.change(field(), { target: { value: key } });
  view.rerender(<div><Model enabled away /></div>);
  expect(field().value).toBe(''); expect(invoke).not.toHaveBeenCalled();
});
test.each([
  ['modelCredentialRejected', "Anthropic didn't accept this key"],
  ['modelProviderUnreachable', "local cluster couldn't reach Anthropic"],
  ['modelModelUnavailable', "isn't listed for this key"],
  ['modelKeyCleanup', 'removal has not been confirmed'],
])('finite %s error cannot echo the company response or key', async (kind, message) => {
  invoke.mockRejectedValue({ kind, message: key, diagnostic: { stderr: key } });
  render(<Model enabled />); keyChoice(); fireEvent.click(screen.getByRole('button', { name: 'Anthropic' }));
  fireEvent.change(field(), { target: { value: key } }); fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect((await screen.findByRole('alert')).textContent).toContain(message);
  if (kind === 'modelKeyCleanup') {
    const retry = screen.getByRole('button', { name: 'Retry removal' });
    const row = retry.closest('.workspace-step-actions')!;
    expect([...row.querySelectorAll('button')].map(button => button.textContent)).toEqual(['Retry removal', 'Copy details for help']);
    expect(retry.classList.contains('button-primary')).toBe(true);
    expect(screen.queryByLabelText('API key')).toBeNull();
  } else expect(field().value).toBe('');
  expect(document.body.textContent).not.toContain(key);
  if (kind === 'modelCredentialRejected') expect(screen.queryByRole('button', { name: 'Remove key' })).toBeNull();
});
test('a failed reopened key connection uses the saved company rather than Copilot recovery', async () => {
  invoke.mockRejectedValue({ kind: 'modelCredentialRejected', vendor: 'anthropic', message: key });
  render(<Model enabled />); fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect((await screen.findByRole('alert')).textContent).toContain("Anthropic didn't accept this key");
  expect(screen.getByRole('status').textContent).toContain('saved key');
  expect(document.body.textContent).not.toContain('GitHub CLI stays signed in');
});
test('reopened connection uses native company metadata, not the initial Copilot choice', async () => {
  invoke.mockResolvedValue({ ready: true, connection: anthropic });
  render(<Model enabled />); fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText(/Anthropic with your API key/);
  expect(screen.getByRole('button', { name: 'Remove key' })).toBeTruthy();
});
test('Change disconnects first, blocks replacement until acknowledgment and keeps retry on failure', async () => {
  invoke.mockResolvedValueOnce({ ready: true, connection: anthropic });
  render(<Model enabled />); fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText(/Anthropic with your API key/);
  let reject!: (e: unknown) => void;
  invoke.mockImplementation(() => new Promise((_, no) => { reject = no; }));
  fireEvent.click(screen.getByRole('button', { name: 'Change' }));
  expect(invoke.mock.calls[1]).toEqual(['setup_remove_model']);
  expect(screen.queryByRole('radio', { name: /Your own API key/ })).toBeNull();
  await act(async () => reject({ kind: 'modelKeyCleanup' }));
  expect(screen.getByRole('button', { name: 'Retry removal' })).toBeTruthy();
  invoke.mockResolvedValue({ ready: false }); fireEvent.click(screen.getByRole('button', { name: 'Retry removal' }));
  await screen.findByRole('radio', { name: /Your own API key/ });
  expect(screen.queryByText(/Anthropic with your API key/)).toBeNull();
});
test.each([
  { ready: true, connection: { ...anthropic, model: 'untrusted' } },
  { ready: true, connection: { ...anthropic, vendor: 'invented' } },
  { ready: true, connection: { ...anthropic, key } },
])('unknown connection metadata cannot establish readiness', async reply => {
  invoke.mockResolvedValue(reply); expect(await connectModel()).toMatchObject({ ok: false });
});
