import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Example from './Example';
import Run from './Run';
import Send from '../clusters/Send';
import { agentCall, SAMPLE_DOCUMENT_ID } from './agent-api';
import { runCall, sampleRun, type RunSnapshot } from './run-api';
import { sendCall } from '../clusters/send-api';
const a = '00000000-0000-4000-8000-000000000001';
const b = '00000000-0000-4000-8000-000000000002';
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const pair = { documentId: a, name: 'alpha', agent: '---\nname: alpha\n---\nInstructions A', tests: 'Checks', source };
const idle = { documentId: a, busy: null, state: 'idle', canRun: true, source, report: null, acknowledged: [], pending: null, error: null };
const complete: RunSnapshot = { ...idle, state: 'complete', report: { verdict: 'passed', cases: [{ id: 'check', question: 'What did the page say?', phase: 'Succeeded', answer: 'Only A answer', completedTools: [], assertions: [{ id: 'answer', kind: 'contains', expected: 'checksum', verdict: 'passed' }], verdict: 'passed' }] } };
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
function native() { vi.stubGlobal('isTauri', true); }

test('bound adapters send document identity and reject another owner with identical hashes', async () => {
  native(); const ipc = vi.fn(() => pair); mockIPC(ipc);
  expect((await agentCall('agent_open', undefined, a)).documentId).toBe(a);
  expect(ipc).toHaveBeenLastCalledWith('agent_open', { documentId: a });
  await expect(agentCall('agent_open', undefined, b)).rejects.toThrow();
  mockIPC(() => complete);
  expect((await runCall('agent_run_status', b)).ok).toBe(false);
});
test('sample setup evidence excludes a passing non-sample result', () => {
  expect(sampleRun(complete)).toBeUndefined();
  expect(sampleRun({ ...complete, documentId: SAMPLE_DOCUMENT_ID })?.report?.verdict).toBe('passed');
});
test('foreign busy names the owner without its result, resources or Stop control', async () => {
  native(); mockIPC(command => command.startsWith('agent_run')
    ? { ...idle, documentId: b, canRun: false, busy: { documentId: a, name: 'alpha' } }
    : { documentId: b, view: null, busy: { documentId: a, name: 'alpha' } });
  render(<><Run documentId={b} /><Send documentId={b} place={{ kind: 'thisComputer' }} stale={false} visible remote={false} /></>);
  expect(await screen.findByText('alpha is running a test.')).toBeTruthy();
  expect(await screen.findByText('alpha is sending tests.')).toBeTruthy();
  expect(screen.queryByText('Only A answer')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Stop' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Run tests' }).hasAttribute('disabled')).toBe(true);
});
test('Send report validation keeps document ownership after Run DTO changes', async () => {
  native(); const view = { documentId: a, id: b, targetId: a, providerId: b, targetName: 'team', source,
    model: 'model', agentName: 'submitted', state: 'complete', stage: 'done', report: complete.report, acknowledged: [], pending: null, error: null, storageError: false };
  mockIPC(() => ({ documentId: a, view, busy: null }));
  expect((await sendCall('send_status', { documentId: a })).view?.report?.verdict).toBe('passed');
  await expect(sendCall('send_status', { documentId: b })).rejects.toThrow();
});
test('switch waits for autosave, keeps failed draft, and retry switches without a remount', async () => {
  native(); let fail = true; const writes: string[] = [];
  mockIPC((command, args) => {
    const request = args as { documentId: string; content: string };
    if (command === 'agent_open') return request.documentId === a ? pair : { ...pair, documentId: b, name: 'beta', agent: 'Instructions B' };
    if (command === 'agent_select') return null;
    if (command === 'agent_save') { writes.push(request.documentId); if (fail) throw Error('save failed'); return { ...pair, agent: request.content }; }
    if (command.startsWith('agent_run')) return { ...idle, documentId: request.documentId };
    if (command === 'send_status') return { documentId: request.documentId, view: null, busy: null };
    throw Error('unexpected');
  });
  const user = userEvent.setup(); const view = render(<Example documentId={a} />);
  await user.click(await screen.findByRole('button', { name: 'File' }));
  const editor = screen.getByRole('textbox', { name: 'Agent file' });
  fireEvent.change(editor, { target: { value: 'Keep A draft' } });
  view.rerender(<Example documentId={b} />);
  await screen.findByText('Not saved');
  expect((editor as HTMLTextAreaElement).value).toBe('Keep A draft');
  expect(screen.queryByText('Instructions B')).toBeNull();
  expect(screen.getByRole('textbox', { name: 'Agent file' })).toBe(editor);
  fail = false; await user.click(screen.getByRole('button', { name: 'Try saving again' }));
  await waitFor(() => expect((screen.getByRole('textbox', { name: 'Agent file' }) as HTMLTextAreaElement).value).toBe('Instructions B'));
  expect(writes.every(id => id === a)).toBe(true);
  expect(screen.getByRole('heading', { name: 'beta.agent.md' })).toBeTruthy();
});
test('late open and reload replies cannot overwrite an identical-byte document', async () => {
  native(); let reload!: (value: unknown) => void; let opens = 0;
  const target = { id: a, context_id: a, context_name: 'team', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'image' };
  const provider = { id: b, name: 'model', ready: true };
  mockIPC((command, args) => {
    const request = args as { documentId: string };
    if (command === 'agent_open') {
      if (request.documentId === a && ++opens > 1) return new Promise(resolve => { reload = resolve; });
      return { ...pair, documentId: request.documentId, name: request.documentId === a ? 'alpha' : 'beta' };
    }
    if (command === 'send_status') return { documentId: request.documentId, view: null, busy: null };
    if (command === 'send_preflight') return { documentId: request.documentId, targetId: a, providerId: b, source: { ...source, agent: 'c'.repeat(64) }, model: 'model', canSend: true, rows: [{ label: 'Source', status: 'allowed', message: 'Captured' }] };
    return { ...idle, documentId: request.documentId };
  });
  const view = render(<Example documentId={a} remote place={{ kind: 'cluster', target, providers: [provider], providerId: b }} />);
  await userEvent.click(await screen.findByRole('button', { name: 'Reload saved files' }));
  await waitFor(() => expect(reload).toBeDefined());
  view.rerender(<Example documentId={b} />);
  await userEvent.click(await screen.findByRole('button', { name: 'File' }));
  await screen.findByRole('heading', { name: 'beta.agent.md' });
  await act(async () => reload({ ...pair, agent: 'Obsolete A reload' }));
  expect((screen.getByRole('textbox', { name: 'Agent file' }) as HTMLTextAreaElement).value).toBe(pair.agent);
});

test('a delayed Run poll from A cannot replace B after a prop change', async () => {
  native(); let resolve!: (value: unknown) => void;
  mockIPC((_command, args) => (args as { documentId: string }).documentId === a
    ? new Promise(done => { resolve = done; }) : { ...idle, documentId: b });
  const view = render(<Run documentId={a} />);
  view.rerender(<Run documentId={b} />);
  await act(async () => resolve(complete));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests' }).hasAttribute('disabled')).toBe(false));
  expect(screen.queryByText('Only A answer')).toBeNull();
});
