import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Model from './Model';
const { invoke, isTauri } = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri }));
beforeEach(() => { invoke.mockReset(); isTauri.mockReturnValue(true); });
afterEach(cleanup);
const anthropic = { kind: 'key', vendor: 'anthropic', model: 'claude-sonnet-5-5' };
const removals = () => invoke.mock.calls.filter(([cmd]) => cmd === 'setup_remove_model');

test('the heading can take programmatic focus', () => {
  render(<Model enabled />);
  expect(screen.getByRole('heading', { name: 'Connect the AI' }).getAttribute('tabindex')).toBe('-1');
});

test('a provider change request on a saved sign-in opens the same removal confirmation and never removes directly', async () => {
  invoke.mockResolvedValueOnce({ ready: true, connection: { kind: 'copilot', model: 'gpt-5.6-terra' } }).mockResolvedValue({ ready: false });
  const view = render(<Model enabled changeRequest={0} />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText(/^Connected$/);
  view.rerender(<Model enabled changeRequest={1} />);
  expect(screen.getByRole('group', { name: 'Change the AI' })).toBeTruthy();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Remove saved sign-in' }));
  expect(removals()).toHaveLength(0);
  fireEvent.click(screen.getByRole('button', { name: 'Remove saved sign-in' }));
  fireEvent.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  await screen.findByText('Saved sign-in removed.');
  expect(removals()).toHaveLength(1);
  expect(screen.getByRole('radio', { name: /Your own API key/ })).toBeTruthy();
});

test('a provider change request without a sign-in focuses the connection choice', () => {
  const view = render(<Model enabled changeRequest={0} />);
  view.rerender(<Model enabled changeRequest={1} />);
  expect(document.activeElement).toBe(screen.getByRole('radio', { name: 'GitHub Copilot' }));
  expect(screen.queryByRole('group', { name: /confirmation/ })).toBeNull();
  expect(invoke).not.toHaveBeenCalled();
});

test('an earlier request does not act on mount, and a busy card only takes focus on its heading', () => {
  invoke.mockImplementation(() => new Promise(() => {}));
  const view = render(<Model enabled changeRequest={2} />);
  expect(document.activeElement).toBe(document.body);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  view.rerender(<Model enabled changeRequest={3} />);
  expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Connect the AI' }));
  expect(screen.queryByRole('group', { name: /confirmation/ })).toBeNull();
  expect(invoke).toHaveBeenCalledTimes(1);
});

test('a hidden card never takes focus for a change request', () => {
  const view = render(<div hidden><Model enabled changeRequest={0} /></div>);
  view.rerender(<div hidden><Model enabled changeRequest={1} /></div>);
  expect(document.activeElement).toBe(document.body);
});

test('Change opens a panel before the saved key removal confirmation', async () => {
  invoke.mockResolvedValueOnce({ ready: true, connection: anthropic });
  render(<Model enabled />);
  fireEvent.click(screen.getByRole('button', { name: 'Connect the AI' }));
  const change = await screen.findByRole('button', { name: 'Change' });
  change.focus(); fireEvent.click(change);
  expect(removals()).toHaveLength(0);
  expect(screen.queryByRole('group', { name: 'Remove key confirmation' })).toBeNull();
  const remove = screen.getByRole('button', { name: 'Remove saved key' });
  expect(remove.closest('[aria-label="Change the AI"]')).toBeTruthy();
  remove.focus(); fireEvent.click(remove);
  expect(screen.getByRole('group', { name: 'Remove key confirmation' })).toBeTruthy();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Cancel' }));
  invoke.mockResolvedValue({ ready: false });
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Remove key' })); });
  expect(removals()).toHaveLength(1);
});
