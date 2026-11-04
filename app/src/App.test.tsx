import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import App from './App';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

test('navigation_order', () => {
  render(<App />);
  const nav = screen.getByRole('navigation', { name: 'Main' });
  expect(within(nav).getAllByRole('button').map(button => button.textContent))
    .toEqual(['Overview', 'Designer', 'Clusters']);
  expect(screen.getByRole('img', { name: 'Orka' })).toBeTruthy();
  expect(screen.getByText('Bridge')).toBeTruthy();
});

test('selected_heading', async () => {
  const user = userEvent.setup();
  render(<App />);
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('Overview');
  for (const name of ['Designer', 'Clusters', 'Overview']) {
    await user.click(screen.getByRole('button', { name }));
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
    expect(screen.getByRole('main', { name })).toBeTruthy();
    const selected = screen.getAllByRole('button').filter(button => button.getAttribute('aria-current') === 'page');
    expect(selected).toEqual([screen.getByRole('button', { name })]);
  }
});

test('keyboard_activation', async () => {
  const user = userEvent.setup();
  render(<App />);
  await user.tab();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Overview' }));
  await user.tab();
  await user.keyboard(' ');
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('Designer');
  await user.tab();
  await user.keyboard('{Enter}');
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('Clusters');
});

test('honest_empty_states', async () => {
  const user = userEvent.setup();
  render(<App />);
  for (const name of ['Overview', 'Designer', 'Clusters']) {
    await user.click(screen.getByRole('button', { name }));
    const main = screen.getByRole('main', { name });
    expect(within(main).getByText('Nothing is connected yet.')).toBeTruthy();
    expect(within(main).getByRole('heading', { level: 2 }).textContent).toBeTruthy();
    expect(within(main).queryByRole('button')).toBeNull();
    expect(within(main).queryByRole('link')).toBeNull();
  }
});

test('no_legacy_host_ipc', async () => {
  const invoke = vi.fn();
  const fetch = vi.fn();
  vi.stubGlobal('__TAURI_INTERNALS__', { invoke });
  vi.stubGlobal('fetch', fetch);
  render(<App />);
  await userEvent.setup().click(screen.getByRole('button', { name: 'Clusters' }));
  await new Promise(resolve => setTimeout(resolve, 300));
  expect(invoke).not.toHaveBeenCalled();
  expect(fetch).not.toHaveBeenCalled();
});
