import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import EngineStatus from './EngineStatus';

afterEach(() => { cleanup(); clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

test('shows unavailable without a native bridge', async () => {
  render(<EngineStatus />);
  expect(await screen.findByRole('heading', { name: 'Engine unavailable' })).toBeTruthy();
  expect(screen.getByText('The native bridge is not available in this preview.')).toBeTruthy();
});

test.each([
  ['unavailable', 'Engine unavailable', 'The engine is not available in this app session.'],
  ['execution', 'Engine error', 'The engine request could not be completed.'],
  ['unsupported', 'Engine error', 'This engine response is not supported.'],
  ['invalidResponse', 'Engine error', 'The engine returned an invalid response.'],
  ['unknown', 'Engine error', 'The engine returned an invalid response.'],
])('renders safe %s state without rejected details or logs', async (kind, heading, message) => {
  vi.stubGlobal('isTauri', true);
  const logs = [vi.spyOn(console, 'log'), vi.spyOn(console, 'warn'), vi.spyOn(console, 'error')];
  mockIPC(() => Promise.reject({ kind, message: 'SECRET_REJECTED', diagnostics: 'SECRET_STDERR' }));
  render(<EngineStatus />);
  expect(await screen.findByRole('heading', { name: heading })).toBeTruthy();
  expect(screen.getByText(message)).toBeTruthy();
  expect(document.body.textContent).not.toContain('SECRET_');
  logs.forEach(log => expect(log).not.toHaveBeenCalled());
});

test('renders only the static diagnostic omission note', async () => {
  vi.stubGlobal('isTauri', true);
  const logs = [vi.spyOn(console, 'log'), vi.spyOn(console, 'warn'), vi.spyOn(console, 'error')];
  mockIPC(() => ({
    kind: 'version', version: 'v0.4.2', hasDiagnostics: true,
    diagnostics: 'SECRET_DIAGNOSTICS', stderr: 'SECRET_STDERR', metadata: { token: 'SECRET_METADATA' },
  }));
  render(<EngineStatus />);
  expect(await screen.findByRole('heading', { name: 'Engine available' })).toBeTruthy();
  expect(screen.getByText('Engine diagnostics were omitted.')).toBeTruthy();
  expect(document.body.textContent).not.toContain('SECRET_');
  logs.forEach(log => expect(log).not.toHaveBeenCalled());
});

test('ignores a late reply after unmount before reading its display fields', async () => {
  vi.stubGlobal('isTauri', true);
  let finish!: (value: unknown) => void;
  mockIPC(() => new Promise(resolve => { finish = resolve; }));
  const { unmount } = render(<EngineStatus />);
  unmount();
  const version = vi.fn(() => 'v0.4.2');
  await act(async () => {
    finish({ kind: 'version', get version() { return version(); }, hasDiagnostics: false });
  });
  expect(version).not.toHaveBeenCalled();
  expect(document.body.textContent).toBe('');
});

test('shows loading then the engine version without claiming cluster readiness', async () => {
  vi.stubGlobal('isTauri', true);
  let finish!: (value: unknown) => void;
  mockIPC(() => new Promise(resolve => { finish = resolve; }));
  render(<EngineStatus />);
  expect(screen.getByRole('heading', { name: 'Checking engine' })).toBeTruthy();
  await act(async () => { finish({ kind: 'version', version: 'v0.4.2', hasDiagnostics: false }); });
  expect(screen.getByRole('heading', { name: 'Engine available' })).toBeTruthy();
  expect(screen.getByText('Version v0.4.2. Cluster readiness has not been checked.')).toBeTruthy();
  expect(screen.queryByRole('button')).toBeNull();
});
