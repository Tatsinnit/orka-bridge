import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Model from './Model';
import { connectKey, connectModel } from './modelState';
const { invoke, isTauri } = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri }));
beforeEach(() => { invoke.mockReset(); isTauri.mockReturnValue(true); });
afterEach(cleanup);
const copilot = { kind: 'copilot', model: 'gpt-5.6-terra' };
const card = () => screen.getByRole('region', { name: 'Connect the AI' });
function keyChoice() { fireEvent.click(screen.getByRole('radio', { name: /Your own API key/ })); }

test('a connected Copilot sign-in shows the fixed model and the account verified when it was saved', async () => {
  invoke.mockResolvedValue({ ready: true, connection: { ...copilot, account: 'octo-cat1' } });
  render(<Model enabled />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText(/^Connected$/);
  expect(card().textContent).toContain('GitHub Copilot');
  expect(screen.getByText('gpt-5.6-terra').tagName).toBe('CODE');
  expect(card().textContent).toContain('Saved GitHub sign-in: octo-cat1');
  expect(card().textContent).not.toContain('account unavailable');
});

test('a saved sign-in without verified account metadata says so instead of naming another account', async () => {
  invoke.mockResolvedValue({ ready: true, connection: copilot });
  render(<Model enabled />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText(/^Connected$/);
  expect(card().textContent).toContain('Saved GitHub sign-in: account unavailable');
  expect(screen.getByText('gpt-5.6-terra')).toBeTruthy();
});

test('account metadata is accepted only as a valid GitHub login on a Copilot connection', async () => {
  invoke.mockResolvedValue({ ready: true, connection: { ...copilot, account: 'a'.repeat(39) } });
  expect(await connectModel()).toMatchObject({ ok: true, connection: { account: 'a'.repeat(39) } });
  for (const account of ['', '-leading', 'two words', 'a'.repeat(40), 'name\n', 'ünicode', 7, null]) {
    invoke.mockResolvedValue({ ready: true, connection: { ...copilot, account } });
    expect(await connectModel()).toMatchObject({ ok: false });
  }
  invoke.mockResolvedValue({ ready: true, connection: { kind: 'key', vendor: 'openai', model: 'gpt-4.1', account: 'octocat' } });
  expect(await connectModel('openai')).toMatchObject({ ok: false });
  expect(await connectKey('openai', 'sk-synthetic-only')).toMatchObject({ ok: false });
});

test('before connecting, each choice names its actual fixed default model without a picker or other providers', () => {
  render(<Model enabled />);
  expect(card().textContent).toContain('Model gpt-5.6-terra, a good default.');
  expect(screen.getByText('gpt-5.6-terra').tagName).toBe('CODE');
  expect(screen.queryByRole('combobox')).toBeNull();
  expect(screen.queryByRole('listbox')).toBeNull();
  expect(card().textContent).not.toMatch(/Azure/);
  // A pre-connection screen never claims to know whose sign-in will be saved.
  expect(card().textContent).not.toMatch(/Saved GitHub sign-in|signed in as/i);
  keyChoice();
  expect(card().textContent).toContain('Model claude-sonnet-5-5, a good default.');
  fireEvent.click(screen.getByRole('button', { name: 'OpenAI' }));
  expect(card().textContent).toContain('Model gpt-4.1, a good default.');
  expect(card().textContent).not.toMatch(/Azure/);
});

test('key custody copy is neutral about the native store and names the workspace', () => {
  render(<Model enabled />); keyChoice();
  const help = card().querySelector('#model-key-help')!.textContent!;
  expect(help).toBe('Stored on this computer, never in your agent files.');
  expect(card().textContent).not.toMatch(/Keychain|Credential Manager|local cluster/);
});

test('a key cannot be queued; the card says it can be entered only once the workspace is ready', () => {
  render(<Model enabled={false} queueable />); keyChoice();
  expect(card().textContent).toContain("Bridge can't queue a key. Enter it once the workspace is ready.");
  expect((screen.getByLabelText('API key') as HTMLInputElement).disabled).toBe(true);
  expect(screen.getByRole('button', { name: 'Connect the AI' }).hasAttribute('disabled')).toBe(true);
  expect(card().textContent).not.toMatch(/local cluster|Queued/);
  expect(invoke).not.toHaveBeenCalled();
});

test('a queued Copilot connection is a status row with a secondary Cancel, separate from the choice', () => {
  render(<Model enabled={false} queueable />);
  expect(card().textContent).toContain('Choose now to connect automatically when the workspace is ready.');
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  const status = screen.getByRole('status');
  expect(status.textContent).toContain('Queued');
  expect(status.textContent).toContain('Connects when the workspace is ready.');
  expect(status.closest('fieldset')).toBeNull();
  const cancel = screen.getByRole('button', { name: 'Cancel queued connection' });
  expect(cancel.textContent).toBe('Cancel');
  expect(cancel.classList.contains('button-secondary')).toBe(true);
  expect(card().textContent).not.toMatch(/local cluster/);
  fireEvent.click(cancel);
  expect(screen.queryByText('Queued')).toBeNull();
  expect(invoke).not.toHaveBeenCalled();
});

test('locked and failure copy names the workspace rather than the local cluster', async () => {
  const view = render(<Model enabled={false} />);
  expect(card().textContent).toContain('Start the workspace before connecting the AI.');
  view.rerender(<Model enabled />);
  expect(card().textContent).toContain('Your sign-in is shared only with your workspace.');
  for (const kind of ['modelGuest', 'modelDelivery', 'modelBusy', 'modelSession', 'modelReconnect', 'modelProviderUnreachable', 'modelConnect']) {
    invoke.mockRejectedValue({ kind });
    const result = await connectModel();
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.message).not.toMatch(/local cluster/);
  }
  invoke.mockRejectedValue({ kind: 'modelGuest' });
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Your workspace and agent files are kept.');
  expect(card().textContent).not.toMatch(/local cluster/);
});
