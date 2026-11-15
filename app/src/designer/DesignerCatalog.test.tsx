import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Example from './Example';
import type { SelectedPlace } from '../clusters/api';

const documentId = '9b2b93c4-375c-4815-9717-39852a96e411';
const pair = { documentId, name: 'reader', agent: '---\nname: reader\nmodel: saved-model\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools: []\n---\nRead.', tests: 'Checks' };
const catalog = (model: string) => ({ defaultModel: model, models: { state: 'ready', items: [{ id: model }] }, tools: { state: 'ready', items: [{ kind: 'builtin', name: 'web_fetch', description: 'Fetch a page' }] } });
const place = (targetId = '19dc68be-f597-4535-8ebd-7759f544b27a', providerId = '29dc68be-f597-4535-8ebd-7759f544b27a'): SelectedPlace => ({ kind: 'cluster', target: { id: targetId, context_id: targetId, context_name: 'team-dev', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'fixture-image' }, providerId });
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });

test.each(['target', 'provider'])('remote catalog is scoped to the selected %s and rejects its late reply', async changed => {
  vi.stubGlobal('isTauri', true); const pending: ((value: unknown) => void)[] = []; const args: unknown[] = [];
  mockIPC((command, values) => {
    if (command === 'agent_open') return pair;
    if (command === 'cluster_catalog') { args.push(values); return new Promise(resolve => pending.push(resolve)); }
    return null;
  });
  const first = place(); const view = render(<Example remote place={first} catalogKey={1} />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await waitFor(() => expect(pending).toHaveLength(1));
  view.rerender(<Example remote place={changed === 'target' ? place('39dc68be-f597-4535-8ebd-7759f544b27a') : place(undefined, '49dc68be-f597-4535-8ebd-7759f544b27a')} catalogKey={1} />);
  await waitFor(() => expect(pending).toHaveLength(2));
  await act(async () => pending[0](catalog('old-model')));
  expect(screen.queryByRole('option', { name: 'old-model' })).toBeNull();
  await act(async () => pending[1](catalog('new-model')));
  expect(screen.getByRole('option', { name: 'new-model' })).toBeTruthy();
  expect((screen.getByRole('combobox', { name: 'It thinks with' }) as HTMLSelectElement).value).toBe('saved-model');
  expect(screen.getByRole('button', { name: 'Add a tool' }).matches(':disabled')).toBe(false);
  expect(Object.keys(args[0] as object).sort()).toEqual(['providerId', 'targetId']);
});

test('failure guidance opens the editable instructions without rerunning tests', async () => {
  vi.stubGlobal('isTauri', true); const runs: string[] = [];
  mockIPC(command => {
    if (command === 'agent_open') return pair;
    if (command === 'agent_run') runs.push(command);
    if (command === 'agent_run_status') return { documentId, busy: null, state: 'complete', canRun: true, modelConnected: true, report: { verdict: 'failed', cases: [{ id: 'reads', question: 'Read a page', phase: 'Succeeded', answer: 'Other answer', completedTools: ['web_fetch'], assertions: [{ id: 'contains', kind: 'contains', expected: 'checksum', verdict: 'failed' }], verdict: 'failed' }] }, acknowledged: [], pending: null, error: null };
    return null;
  });
  const user = userEvent.setup(); render(<Example />);
  await user.click(await screen.findByRole('button', { name: 'File' }));
  await user.click(await screen.findByRole('button', { name: 'Edit instructions' }));
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'Instructions' })));
  expect(runs).toEqual([]);
});

test.each(['different-model', null])('remote defaults do not claim that a saved model is unavailable (%s)', async defaultModel => {
  vi.stubGlobal('isTauri', true); mockIPC(command => command === 'agent_open' ? pair : command === 'cluster_catalog' ? { ...catalog('different-model'), defaultModel, models: { state: 'ready', items: defaultModel ? [{ id: defaultModel }] : [] } } : null);
  render(<Example remote place={place()} catalogKey={1} />);
  await waitFor(() => expect(screen.getByRole('combobox', { name: 'It thinks with' }).matches(':disabled')).toBe(false));
  expect(screen.getByRole('option', { name: 'saved-model' })).toBeTruthy();
  expect(screen.queryByText(/not listed by the connected AI/)).toBeNull();
  if (defaultModel === null) expect(screen.getByText(/No default model is configured for this AI connection/)).toBeTruthy();
});

test('a restored active Send keeps its completed result and unconfirmed recovery visible', async () => {
  vi.stubGlobal('isTauri', true); let state = 'running';
  const report = { verdict: 'passed', cases: [{ id: 'reads', question: 'Read', phase: 'Succeeded', answer: 'Saved completed answer', completedTools: [], verdict: 'passed', assertions: [{ id: 'text', kind: 'contains', expected: 'answer', verdict: 'passed' }] }] };
  mockIPC(command => command === 'agent_open' ? pair : command === 'send_history' ? [] : command === 'send_status' ? { documentId, busy: state === 'running' ? { documentId, name: 'reader' } : null, view: { documentId, id: documentId, targetId: documentId, providerId: documentId, targetName: 'team-dev', source: null, model: null, agentName: null, state, stage: state === 'running' ? 'capture' : 'done', acknowledged: [], pending: null, report: state === 'complete' ? report : null, error: null, storageError: false, cancellation: state === 'unknown' ? 'unconfirmed' : null } } : null);
  render(<Example />);
  const panel = await screen.findByRole('region', { name: 'Send and cluster tests' });
  state = 'complete';
  await waitFor(() => expect(panel.textContent).toContain('Saved completed answer'));
  expect(screen.getByRole('region', { name: 'Send and cluster tests' })).toBe(panel);
  expect(screen.getByRole('region', { name: 'Past Sends' }).closest('[hidden]')).toBeNull();
  state = 'unknown';
  await waitFor(() => expect(panel.textContent).toContain('Cancellation not confirmed'));
  expect(screen.getByRole('region', { name: 'Send and cluster tests' })).toBe(panel);
});

test('Personality offers an example without inserting it in the agent file', async () => {
  vi.stubGlobal('isTauri', true); mockIPC(command => command === 'agent_open' ? pair : null);
  const user = userEvent.setup(); render(<Example />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await user.click(screen.getByRole('button', { name: 'More settings' }));
  const field = screen.getByRole('textbox', { name: 'Personality' }) as HTMLTextAreaElement;
  expect(field.placeholder).toContain('Be concise'); expect(field.value).toBe('');
});
