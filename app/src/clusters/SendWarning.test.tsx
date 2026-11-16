import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import type { ComponentProps } from 'react';
import type { RunSnapshot } from '../designer/run-api';
import Send from './Send';

const id = '9b2b93c4-375c-4815-9717-39852a96e411';
const target = { id: '00000000-0000-4000-8000-000000000002', context_id: '00000000-0000-4000-8000-000000000001', context_name: 'team-dev', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'ghcr.io/orka:0.9.4' };
const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'team-model', ready: true };
const place = { kind: 'cluster' as const, target, providers: [provider], providerId: provider.id };
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const allowed = { documentId: id, source, targetId: target.id, providerId: provider.id, model: 'literal-model', rows: [{ label: 'AI connection', status: 'allowed', message: 'Ready.' }], canSend: true, action: 'create', canSeparate: false };
const failedCase = { id: 'reads-the-title', question: 'What is the title?', phase: 'Succeeded', answer: 'No', completedTools: [], assertions: [{ id: 'has', kind: 'contains' as const, expected: 'Title', verdict: 'failed' as const }], verdict: 'failed' as const };
const local: RunSnapshot = { documentId: id, busy: null, state: 'complete', canRun: true, report: { verdict: 'failed', cases: [failedCase] }, acknowledged: [], pending: null, error: null, source, sourceCurrent: true };
beforeEach(() => {
  // jsdom has no top-layer popover; the real light dismissal arrives as a toggle event.
  Object.defineProperty(HTMLDivElement.prototype, 'showPopover', { configurable: true, value() { this.style.display = 'grid'; } });
});
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); Reflect.deleteProperty(HTMLDivElement.prototype, 'showPopover'); });
function native(check: object = allowed) {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn<(command: string, args?: unknown) => unknown>(command => {
    if (command === 'send_status') return { documentId: id, view: null, busy: null };
    if (command === 'send_history') return [];
    if (command === 'send_preflight') return check;
    if (command === 'send_start') return { documentId: id, view: null, busy: null };
    throw Error(`Unsupported ${command}`);
  });
  mockIPC(ipc); return ipc;
}
const sends = (ipc: ReturnType<typeof native>) => ipc.mock.calls.filter(([command]) => command === 'send_start').length;
async function ready(props: Partial<ComponentProps<typeof Send>> = {}) {
  const view = render(<Send place={place} source={source} stale={false} local={local} {...props} />);
  const send = await screen.findByRole('button', { name: 'Send and run tests' }) as HTMLButtonElement;
  await waitFor(() => expect(send.disabled).toBe(false));
  return { ...view, send };
}
test('a failing local test warns from the Send action itself, with Fix it first as the focused primary', async () => {
  const ipc = native(); const user = userEvent.setup(); const { send } = await ready();
  expect(screen.queryByRole('button', { name: 'Send to a cluster' })).toBeNull();
  await user.click(send);
  const warning = await screen.findByRole('dialog', { name: 'Local test failed' });
  expect(warning.textContent).toContain('Local test ‘reads-the-title’ failed. You can still send this agent.');
  const fix = screen.getByRole('button', { name: 'Fix it first' });
  expect(fix.className).toContain('primary');
  expect(document.activeElement).toBe(fix);
  expect(screen.getByRole('button', { name: 'Send anyway' }).className).not.toContain('primary');
  expect(sends(ipc)).toBe(0);
});
test('Escape and light dismissal return to Send without sending', async () => {
  const ipc = native(); const user = userEvent.setup(); const { send } = await ready();
  await user.click(send); await screen.findByRole('dialog', { name: 'Local test failed' });
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog', { name: 'Local test failed' })).toBeNull();
  expect(document.activeElement).toBe(send);
  await user.click(send);
  const warning = await screen.findByRole('dialog', { name: 'Local test failed' });
  act(() => { fireEvent(warning, Object.assign(new Event('toggle'), { newState: 'closed', oldState: 'open' })); });
  expect(screen.queryByRole('dialog', { name: 'Local test failed' })).toBeNull();
  await waitFor(() => expect(document.activeElement).toBe(send));
  expect(sends(ipc)).toBe(0);
});
test('Fix it first sends no request and hands the failing test to the editor flow; Send anyway uses the same send', async () => {
  const ipc = native(); const user = userEvent.setup(); const onFixLocal = vi.fn();
  const { send } = await ready({ onFixLocal });
  await user.click(send); await user.click(await screen.findByRole('button', { name: 'Fix it first' }));
  expect(onFixLocal).toHaveBeenCalledWith('reads-the-title');
  expect(screen.queryByRole('dialog')).toBeNull(); expect(sends(ipc)).toBe(0);
  await user.click(send); await user.click(await screen.findByRole('button', { name: 'Send anyway' }));
  await waitFor(() => expect(sends(ipc)).toBe(1));
  expect(ipc.mock.calls.find(([command]) => command === 'send_start')?.[1]).toMatchObject({ expectedSource: source });
});
test('the warning is placed with JS coordinates inside a 540px viewport', async () => {
  native(); const user = userEvent.setup();
  vi.stubGlobal('innerWidth', 540); vi.stubGlobal('innerHeight', 900);
  const { send } = await ready();
  send.getBoundingClientRect = () => ({ left: 500, right: 540, top: 800, bottom: 832, width: 40, height: 32, x: 500, y: 800, toJSON() { return {}; } });
  await user.click(send);
  const warning = await screen.findByRole('dialog', { name: 'Local test failed' });
  const left = parseFloat(warning.style.left), width = parseFloat(warning.style.width);
  expect(left).toBeGreaterThanOrEqual(8); expect(left + width).toBeLessThanOrEqual(532);
  expect(parseFloat(warning.style.top)).toBeGreaterThanOrEqual(8);
});
test('a blocked, changed or saving state closes the warning instead of leaving Send anyway enabled', async () => {
  native(); const user = userEvent.setup();
  const { send, rerender } = await ready();
  await user.click(send); await screen.findByRole('dialog', { name: 'Local test failed' });
  rerender(<Send place={place} source={source} stale={false} local={local} blocked="Save your changes first" />);
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Send anyway' })).toBeNull();
  expect(document.activeElement).toBe(screen.getByRole('region', { name: 'Send and cluster tests' }));
});
test.each([true, false])('a separate-agent Send warns and preserves its intent when ordinary Send is allowed=%s', async canSend => {
  const ipc = native({ ...allowed, canSend, canSeparate: true, action: canSend ? 'create' : 'blocked' });
  const user = userEvent.setup(); const onFixLocal = vi.fn();
  render(<Send place={place} source={source} stale={false} local={local} onFixLocal={onFixLocal} />);
  const separate = await screen.findByRole('button', { name: 'Send as a separate agent' });
  await user.click(separate);
  await screen.findByRole('dialog'); expect(sends(ipc)).toBe(0);
  await user.click(screen.getByRole('button', { name: 'Fix it first' }));
  expect(onFixLocal).toHaveBeenCalledWith(failedCase.id); expect(sends(ipc)).toBe(0);
  await user.click(separate);
  await user.click(await screen.findByRole('button', { name: 'Send anyway' }));
  await waitFor(() => expect(sends(ipc)).toBe(1));
  expect(ipc.mock.calls.find(([command]) => command === 'send_start')?.[1]).toMatchObject({ expectedSource: source, separateAgent: true });
});
test('an open warning is repositioned inside the viewport after narrowing the window', async () => {
  native(); const user = userEvent.setup();
  vi.stubGlobal('innerWidth', 1280); vi.stubGlobal('innerHeight', 900);
  const { send } = await ready();
  send.getBoundingClientRect = () => ({ left: 1000, right: 1120, top: 600, bottom: 632, width: 120, height: 32, x: 1000, y: 600, toJSON() { return {}; } });
  await user.click(send);
  const warning = await screen.findByRole('dialog');
  expect(parseFloat(warning.style.left)).toBeGreaterThan(540);
  vi.stubGlobal('innerWidth', 540);
  fireEvent(window, new Event('resize'));
  await waitFor(() => expect(parseFloat(warning.style.left) + parseFloat(warning.style.width)).toBeLessThanOrEqual(532));
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Fix it first' }));
});
test('a stale or changed local result sends directly, with no warning step', async () => {
  const ipc = native(); const user = userEvent.setup();
  const { send, rerender } = await ready({ stale: true });
  await user.click(send); await waitFor(() => expect(sends(ipc)).toBe(1));
  rerender(<Send place={place} source={source} stale={false} local={{ ...local, source: { ...source, agent: 'c'.repeat(64) } }} />);
  await user.click(send); await waitFor(() => expect(sends(ipc)).toBe(2));
  expect(screen.queryByRole('dialog')).toBeNull();
});
