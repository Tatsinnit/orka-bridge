import { StrictMode } from 'react';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Example from './Example';

const a = '00000000-0000-4000-8000-000000000001';
const b = '00000000-0000-4000-8000-000000000002';
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const pair = (documentId: string) => ({ documentId, name: documentId === a ? 'alpha' : 'beta', agent: `---\nname: ${documentId === a ? 'alpha' : 'beta'}\n---\nInstructions`, tests: 'Checks', source });
const idle = (documentId: string) => ({ documentId, busy: null, state: 'idle', canRun: true, source, report: null, acknowledged: [], pending: null, error: null });
function native(select: () => unknown = () => null, save: () => unknown = () => { throw Error('save failed'); }, onRun?: () => void) {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn((command: string, args: unknown) => {
    const request = args as { documentId: string };
    if (command === 'agent_open') return pair(request.documentId);
    if (command === 'agent_select') return select();
    if (command === 'agent_save') return save();
    if (command === 'agent_run') onRun?.();
    if (command.startsWith('agent_run')) return idle(request.documentId);
    if (command === 'send_status') return { documentId: request.documentId, view: null, busy: null };
    if (command === 'agent_chat_status') return { documentId: request.documentId, state: 'idle', messages: [] };
    return null;
  });
  mockIPC(ipc); return ipc;
}
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });

test('Overview Chat and Open select the requested Designer view after guarded selection', async () => {
  native(); const handled = vi.fn();
  const view = render(<Example documentId={a} actionRequest={{ id: 1, documentId: a, action: 'chat' }} onActionHandled={handled} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Chat', pressed: true })).toBeTruthy());
  expect(handled).toHaveBeenCalledWith(1);
  view.rerender(<Example documentId={a} setupPending actionRequest={{ id: 2, documentId: a, action: 'open' }} onActionHandled={handled} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Agent', pressed: true })).toBeTruthy());
});
test.each(['Chat', 'File'])('Overview Run selects Agent view before starting from %s', async previous => {
  const visibleViews: string[] = [];
  const ipc = native(undefined, undefined, () => { visibleViews.push(screen.getByRole('button', { name: 'Agent' }).getAttribute('aria-pressed') ?? ''); });
  const view = render(<Example documentId={a} />);
  await userEvent.click(await screen.findByRole('button', { name: previous }));
  view.rerender(<Example documentId={a} actionRequest={{ id: 1, documentId: a, action: 'run' }} />);
  await waitFor(() => expect(ipc.mock.calls.filter(([cmd]) => cmd === 'agent_run')).toHaveLength(1));
  expect(visibleViews).toEqual(['true']);
  expect(screen.getByRole('button', { name: 'Agent', pressed: true })).toBeTruthy();
});

test.each([false, true])('same-document Overview Run waits for its new selection request (reject: %s)', async reject => {
  let finish!: () => void;
  const ipc = native(() => new Promise<void>((resolve, fail) => { finish = () => reject ? fail(Error('selection failed')) : resolve(); }));
  const view = render(<Example documentId={a} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests' }).hasAttribute('disabled')).toBe(false));
  view.rerender(<Example documentId={a} openDocumentRequest={1} actionRequest={{ id: 1, documentId: a, action: 'run' }} />);
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'agent_run')).toHaveLength(0);
  await act(async () => finish());
  if (reject) { await screen.findByText('Your agent selection could not be saved. Try choosing it again.'); expect(ipc.mock.calls.filter(([cmd]) => cmd === 'agent_run')).toHaveLength(0); }
  else await waitFor(() => expect(ipc.mock.calls.filter(([cmd]) => cmd === 'agent_run')).toHaveLength(1));
});

test('Overview Run waits for selection, opens only the requested document, and starts once', async () => {
  let selected!: () => void;
  const ipc = native(() => new Promise<void>(resolve => { selected = resolve; }));
  const handled = vi.fn();
  render(<StrictMode><Example documentId={b} persistInitialSelection actionRequest={{ id: 1, documentId: b, action: 'run' }} onActionHandled={handled} /></StrictMode>);
  await waitFor(() => expect(selected).toBeTypeOf('function'));
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'agent_run')).toHaveLength(0);
  await act(async () => selected());
  await waitFor(() => expect(ipc.mock.calls.filter(([cmd]) => cmd === 'agent_run')).toHaveLength(1));
  expect(ipc.mock.calls.find(([cmd]) => cmd === 'agent_run')?.[1]).toEqual({ documentId: b });
  expect(handled).toHaveBeenCalledWith(1);
});
test.each(['setup', 'hidden', 'selection-failed'])('Overview Run is dropped, not replayed after %s recovers', async reason => {
  const ipc = native(() => { if (reason === 'selection-failed') throw Error('selection failed'); return null; });
  const handled = vi.fn(); const request = { id: 1, documentId: b, action: 'run' as const };
  const view = render(<Example documentId={b} persistInitialSelection setupPending={reason === 'setup'} active={reason !== 'hidden'} actionRequest={request} onActionHandled={handled} />);
  await waitFor(() => expect(handled).toHaveBeenCalledWith(1));
  view.rerender(<Example documentId={b} actionRequest={request} onActionHandled={handled} />);
  await screen.findByRole('heading', { name: 'beta' });
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'agent_run')).toHaveLength(0);
});
test('a failed autosave cannot run the old document and does not replay after recovery', async () => {
  const ipc = native(); const handled = vi.fn();
  const view = render(<Example documentId={a} />);
  await userEvent.click(await screen.findByRole('button', { name: 'File' }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Agent file' }), { target: { value: 'Unsaved alpha' } });
  view.rerender(<Example documentId={b} actionRequest={{ id: 1, documentId: b, action: 'run' }} onActionHandled={handled} />);
  await screen.findByText('Not saved');
  await waitFor(() => expect(handled).toHaveBeenCalledWith(1));
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'agent_run')).toHaveLength(0);
});
