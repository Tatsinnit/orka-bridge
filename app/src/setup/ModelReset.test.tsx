import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Model from './Model';
import { connectModel } from './modelState';

const { invoke, isTauri } = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri }));
const recovery = {
  installation: '10000000-0000-4000-8000-000000000001', savedInstallation: '20000000-0000-4000-8000-000000000002',
  credentialSlot: '30000000-0000-4000-8000-000000000003',
};
beforeEach(() => { invoke.mockReset(); isTauri.mockReturnValue(true); });
afterEach(cleanup);
async function mismatch() {
  invoke.mockRejectedValueOnce({ kind: 'modelInstallationMismatch', recovery, message: 'SECRET' });
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText("Bridge's saved AI connection is from an earlier setup.");
}

test('mismatch has one primary and confirmed reset, excludes duplicates and returns to provider choice', async () => {
  const onState = vi.fn(); render(<Model enabled onState={onState} />); await mismatch();
  expect(screen.queryByRole('button', { name: 'Check again' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Remove sign-in' })).toBeNull();
  expect(document.querySelectorAll('.button-primary')).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: 'Reset AI connection' }));
  expect(screen.getByRole('group', { name: 'Reset AI connection confirmation' }).textContent).toContain("Your GitHub account and your agents don't change");
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(invoke).toHaveBeenCalledExactlyOnceWith('setup_connect_model');
  fireEvent.click(screen.getByRole('button', { name: 'Reset AI connection' }));
  let finish!: (value: unknown) => void; invoke.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const reset = screen.getByRole('button', { name: 'Reset' }); fireEvent.click(reset); fireEvent.click(reset);
  expect(invoke.mock.calls.filter(([name]) => name === 'setup_reset_model')).toEqual([['setup_reset_model']]);
  expect(screen.getByText('Resetting')).toBeTruthy();
  expect(onState).toHaveBeenLastCalledWith('resetting');
  expect(screen.queryByRole('button', { name: /Remove|Connect the AI/ })).toBeNull();
  await act(async () => { finish({ ready: false }); });
  expect(screen.getByText('Connection reset. Choose the AI again.')).toBeTruthy();
  expect(screen.getByRole('radio', { name: 'GitHub Copilot' }).matches(':disabled')).toBe(false);
  expect(screen.getByRole('button', { name: 'Connect the AI' })).toBeTruthy();
  expect(document.body.textContent).not.toContain('SECRET');
});

test('an explicit provider-change request uses reset confirmation for a mismatched record', async () => {
  const view = render(<Model enabled changeRequest={0} />); await mismatch();
  view.rerender(<Model enabled changeRequest={1} />);
  expect(screen.getByRole('group', { name: 'Reset AI connection confirmation' })).toBeTruthy();
  expect(screen.queryByRole('group', { name: 'Remove sign-in confirmation' })).toBeNull();
  expect(invoke).toHaveBeenCalledExactlyOnceWith('setup_connect_model');
});

test('another provider uses the same reset confirmation and opens key choice only after success', async () => {
  render(<Model enabled />); await mismatch();
  fireEvent.click(screen.getByRole('button', { name: 'Use another provider' }));
  expect(screen.getByText(/Use another provider starts with this same reset/)).toBeTruthy();
  invoke.mockResolvedValue({ ready: false }); fireEvent.click(screen.getByRole('button', { name: 'Reset' }));
  expect(await screen.findByLabelText('API key')).toBeTruthy();
  expect((screen.getByRole('radio', { name: /Your own API key/ }) as HTMLInputElement).checked).toBe(true);
});

test.each(['credentialRemovalUnconfirmed', 'recordRemovalUnconfirmed'])('reset failure distinguishes %s, retries explicitly and copies only safe details', async resetStage => {
  const user = userEvent.setup(); const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  render(<Model enabled />); await mismatch();
  await user.click(screen.getByRole('button', { name: 'Reset AI connection' }));
  invoke.mockRejectedValue({ kind: 'modelStore', recovery: { ...recovery, resetStage }, message: 'SECRET' });
  await user.click(screen.getByRole('button', { name: 'Reset' }));
  expect(await screen.findByRole('heading', { name: "Bridge couldn't reset the AI connection" })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Show details' }));
  expect(screen.getByText(resetStage === 'recordRemovalUnconfirmed' ? 'Native credential removed; saved record removal not confirmed.' : 'Native credential removal not confirmed; saved record kept.')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Copy details for help' }));
  expect(copy.mock.calls[0][0]).toContain('Operation: Resetting AI connection');
  expect(copy.mock.calls[0][0]).toContain(recovery.savedInstallation);
  expect(copy.mock.calls[0][0]).not.toContain('SECRET');
  expect(screen.getByText('Details copied. No keys or sign-in tokens are included.')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Reset again' }));
  expect(await screen.findByText(/[Cc]opy the details for whoever helps you with Bridge/)).toBeTruthy();
  expect(screen.queryByRole('button', { name: /Force|Check again|Remove sign-in/ })).toBeNull();
});

test('a later unclassified reset failure does not reuse an earlier custody confirmation', async () => {
  const user = userEvent.setup(); const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  render(<Model enabled />); await mismatch();
  fireEvent.click(screen.getByRole('button', { name: 'Reset AI connection' }));
  invoke.mockRejectedValueOnce({ kind: 'modelUnavailable', recovery: { ...recovery, resetStage: 'recordRemovalUnconfirmed' } });
  fireEvent.click(screen.getByRole('button', { name: 'Reset' }));
  await screen.findByRole('button', { name: 'Reset again' });
  invoke.mockRejectedValueOnce({ kind: 'modelSession' });
  fireEvent.click(screen.getByRole('button', { name: 'Reset again' }));
  await screen.findByText(/Reset failed again/);
  fireEvent.click(screen.getByRole('button', { name: 'Show details' }));
  expect(screen.queryByText('Native credential removed; saved record removal not confirmed.')).toBeNull();
  expect(screen.getByText('Bridge could not check what is still saved. The identifiers below are from the last confirmed record.')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Copy details for help' }));
  expect(copy.mock.calls[0][0]).toContain('Reset stage: Unconfirmed');
  expect(copy.mock.calls[0][0]).not.toContain('Reset stage: Not started');
});

test('reset completion message disappears when a new connection starts', async () => {
  render(<Model enabled />); await mismatch();
  fireEvent.click(screen.getByRole('button', { name: 'Reset AI connection' }));
  invoke.mockResolvedValueOnce({ ready: false }); fireEvent.click(screen.getByRole('button', { name: 'Reset' }));
  await screen.findByText('Connection reset. Choose the AI again.');
  invoke.mockImplementation(() => new Promise(() => {}));
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect(screen.queryByText('Connection reset. Choose the AI again.')).toBeNull();
});

test('unknown and malformed causes never authorize mismatch reset or display supplied identifiers', async () => {
  for (const bad of [{ kind: 'unknown', recovery }, { kind: 'modelInstallationMismatch', recovery: { ...recovery, installation: 'SECRET' } }]) {
    invoke.mockRejectedValue(bad); const result = await connectModel();
    expect(result).not.toHaveProperty('recovery');
    expect(JSON.stringify(result)).not.toContain('SECRET');
  }
});

test('Remove is not offered during Connect and removal failure names removal', async () => {
  let finish!: (value: unknown) => void;
  invoke.mockImplementation(() => new Promise(resolve => { finish = resolve; })); render(<Model enabled />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect(screen.queryByRole('button', { name: 'Remove sign-in' })).toBeNull();
  await act(async () => { finish({ ready: true }); });
  fireEvent.click(screen.getByRole('button', { name: 'Change' }));
  fireEvent.click(screen.getByRole('button', { name: 'Remove saved sign-in' }));
  invoke.mockRejectedValue({ kind: 'modelBusy' }); fireEvent.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  expect(await screen.findByRole('heading', { name: "Couldn't remove the saved sign-in" })).toBeTruthy();
});

test('place-card recovery keeps provider choice visible when its capture error is cleared', async () => {
  const destination = render(<div data-testid="destination" />).getByTestId('destination');
  invoke.mockRejectedValueOnce({ kind: 'modelCaptureUnavailable' });
  const view = render(<Model enabled onPlaceCard recoveryTarget={destination} />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await within(destination).findByRole('alert');
  fireEvent.click(within(destination).getByRole('radio', { name: /Your own API key/ }));
  expect(within(destination).getByLabelText('API key')).toBeTruthy();
  view.rerender(<Model enabled recoveryTarget={destination} />);
  view.rerender(<Model enabled onPlaceCard recoveryTarget={destination} />);
  expect(destination.childElementCount).toBe(0);
});

test('place-card recovery keeps removal progress and completion in its destination', async () => {
  const destination = render(<div data-testid="destination" />).getByTestId('destination');
  invoke.mockResolvedValueOnce({ ready: true });
  render(<Model enabled onPlaceCard recoveryTarget={destination} />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText('Connected');
  fireEvent.click(screen.getByRole('button', { name: 'Change' }));
  fireEvent.click(screen.getByRole('button', { name: 'Remove saved sign-in' }));
  invoke.mockRejectedValueOnce({ kind: 'modelBusy' });
  fireEvent.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  await within(destination).findByRole('button', { name: 'Retry removal' });
  let finish!: (value: unknown) => void; invoke.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  // The Change panel still renders its own copy of the action (tracked as a production duplicate-render issue); target the standalone one.
  fireEvent.click(within(destination).getAllByRole('button', { name: 'Retry removal' }).find(b => !b.closest('.workspace-change'))!);
  expect(within(destination).getByText('Removing')).toBeTruthy();
  await act(async () => { finish({ ready: false }); });
  expect(within(destination).getByText('Saved sign-in removed.')).toBeTruthy();
});

test('one Model preserves pending reset, details and no stolen focus when its portal destination changes', async () => {
  const one = document.createElement('div'), two = document.createElement('div'); document.body.append(one, two);
  const user = userEvent.setup(); const view = render(<><button>Other control</button><Model enabled recoveryTarget={one} /></>);
  await mismatch(); expect(within(one).getByText(/earlier setup/)).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Show details' }));
  await user.click(screen.getByRole('button', { name: 'Reset AI connection' }));
  let finish!: (value: unknown) => void; invoke.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  await user.click(screen.getByRole('button', { name: 'Reset' }));
  view.rerender(<><button>Other control</button><Model enabled recoveryTarget={two} /></>);
  await user.click(screen.getByRole('button', { name: 'Other control' }));
  await act(async () => { finish({ ready: false }); });
  expect(screen.getByText('Connection reset. Choose the AI again.')).toBeTruthy();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Other control' }));
  expect(invoke.mock.calls.filter(([name]) => name === 'setup_reset_model')).toHaveLength(1);
  view.unmount(); one.remove(); two.remove();
});
