import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, test, vi } from 'vitest';
import Example from './Example';
const id = '9b2b93c4-375c-4815-9717-39852a96e411';
const source = '---\nname: reader\nmodel: listed\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  soul: {inline: old, digest: sha256:old}\n  tools: []\n---\nRead a page.\n';
const pair = { documentId: id, name: 'reader', agent: source, tests: 'Tests stay separate' };
const idle = { documentId: id, state: 'ready', busy: false, canSend: true, versions: [], error: null };
let sent: { documentId: string; message: string; saved: string }[];
function native(save?: (content: string) => unknown) {
  sent = []; let saved = source; vi.stubGlobal('isTauri', true);
  mockIPC(async (command, args) => {
    if (command === 'agent_open') return pair;
    if (command === 'agent_save') {
      const content = (args as { content: string }).content;
      const result = save ? await save(content) : { ...pair, agent: content };
      saved = content; return result;
    }
    if (command === 'agent_chat_send') { sent.push({ ...(args as { documentId: string; message: string }), saved }); return { ...idle, state: 'preparing', busy: true, canSend: false }; }
    if (command.startsWith('agent_chat_')) return idle;
    if (command.startsWith('agent_run')) return { documentId: id, busy: null, state: 'idle', canRun: true, report: null, acknowledged: [], pending: null, error: null };
    if (command === 'send_history') return [];
    if (command === 'send_status') return { documentId: id, view: null, busy: null };
    throw Error(`Unexpected ${command}`);
  });
}
afterEach(() => { cleanup(); clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
async function chat() { const user = userEvent.setup(); await user.click(await screen.findByRole('button', { name: 'Chat' })); await screen.findByRole('textbox', { name: 'Message' }); return user; }
function compose() { fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'A question' } }); }
test('Chat replaces tests and editor without changing their evidence or draft', async () => {
  native(); render(<Example />); const user = await chat(); compose();
  expect(screen.queryByRole('region', { name: 'Tests on this computer' })).toBeNull();
  expect(screen.queryByRole('textbox', { name: 'Instructions' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Send to a cluster' })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Agent' }));
  expect(screen.getByRole('region', { name: 'Tests on this computer' })).toBeTruthy();
  expect((screen.getByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement).value).toContain('Read a page.');
  await user.click(screen.getByRole('button', { name: 'Chat' }));
  expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe('A question');
});
test('a failed external document selection restores the parent and keeps Chat usable for the displayed agent', async () => {
  native(); const settled = vi.fn();
  function Parent() {
    const [documentId, setDocumentId] = useState(id);
    return <><button onClick={() => setDocumentId('a7f34ba2-b9bd-4b55-b69d-f0a84a1bf528')}>Open other agent</button><Example documentId={documentId} onDocument={value => { settled(value); setDocumentId(value); }} /></>;
  }
  render(<Parent />); const user = await chat(); compose();
  await user.click(screen.getByRole('button', { name: 'Open other agent' }));
  await screen.findByText('Your agent selection could not be saved. Try choosing it again.');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(false));
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  expect(sent).toEqual([{ documentId: id, message: 'A question', saved: source }]);
  expect(settled).toHaveBeenCalledExactlyOnceWith(id);
});
test('a pending autosave blocks Chat until its latest bytes are confirmed', async () => {
  let finish!: (value: unknown) => void; let content = '';
  native(value => { content = value; return new Promise(resolve => { finish = resolve; }); });
  const user = userEvent.setup(); render(<Example />);
  fireEvent.change(await screen.findByRole('textbox', { name: 'Instructions' }), { target: { value: 'Changed before Chat' } });
  await chat(); compose(); await screen.findByText('Saving…');
  fireEvent.click(screen.getByRole('button', { name: 'Send message' })); expect(sent).toEqual([]);
  await act(async () => finish({ ...pair, agent: content }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(false));
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  expect(sent).toEqual([{ documentId: id, message: 'A question', saved: content }]); expect(content).toContain('Changed before Chat');
});
test('save failure preserves composed text and the existing retry saves before Chat', async () => {
  let fail = true; native(content => fail ? Promise.reject('private') : { ...pair, agent: content });
  render(<Example />); const user = userEvent.setup();
  fireEvent.change(await screen.findByRole('textbox', { name: 'Instructions' }), { target: { value: 'Needs saving' } });
  await chat(); compose(); await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(true));
  await screen.findByText('Not saved'); expect(sent).toEqual([]);
  expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe('A question');
  fail = false; await user.click(screen.getByRole('button', { name: 'Agent' }));
  await user.click(screen.getByRole('button', { name: 'Try saving again' }));
  await user.click(screen.getByRole('button', { name: 'Chat' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(false));
});
test('switching to Chat keeps asynchronous personality editing mounted and waits for its save', async () => {
  let finish!: (value: ArrayBuffer) => void;
  vi.spyOn(crypto.subtle, 'digest').mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  native(); render(<Example />); const user = userEvent.setup();
  await user.click(await screen.findByText('More settings'));
  fireEvent.change(screen.getByRole('textbox', { name: 'Personality' }), { target: { value: 'New personality' } });
  await chat(); compose(); expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(true);
  await act(async () => finish(new Uint8Array(32).buffer));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(false));
  await user.click(screen.getByRole('button', { name: 'Send message' })); expect(sent[0].saved).toContain('New personality');
});
test('failed personality conversion cannot let Chat silently use the earlier saved personality', async () => {
  vi.spyOn(crypto.subtle, 'digest').mockRejectedValue(Error('hash failed'));
  native(); const view = render(<Example />); const user = userEvent.setup(); await user.click(await screen.findByText('More settings'));
  fireEvent.change(screen.getByRole('textbox', { name: 'Personality' }), { target: { value: 'Unsaved personality' } });
  await screen.findByRole('alert'); await chat(); compose();
  expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(true);
  fireEvent.keyDown(screen.getByRole('textbox', { name: 'Message' }), { key: 'Enter' }); expect(sent).toEqual([]);
  await user.click(screen.getByRole('button', { name: 'Agent' }));
  expect(screen.getByRole('button', { name: 'Run tests' }).hasAttribute('disabled')).toBe(true);
  expect(screen.getByRole('button', { name: 'Send to a cluster' }).hasAttribute('disabled')).toBe(true);
  const other = 'a7f34ba2-b9bd-4b55-b69d-f0a84a1bf528'; view.rerender(<Example documentId={other} />);
  await user.click(screen.getByRole('button', { name: 'Chat' }));
  expect(screen.getByRole('textbox', { name: 'Message' }).id).toBe(`message-${id}`);
  // Returning to the original document and editing File clears the unresolved
  // personality draft through the existing source-change cleanup.
  view.rerender(<Example documentId={id} />);
  await user.click(screen.getByRole('button', { name: 'File' }));
  const editor = screen.getByRole('textbox', { name: 'Agent file' });
  fireEvent.change(editor, { target: { value: source + '\nCorrected in File.' } }); fireEvent.blur(editor);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests' }).hasAttribute('disabled')).toBe(false));
  expect(screen.getByRole('button', { name: 'Send to a cluster' }).hasAttribute('disabled')).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Chat' }));
  expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(false);
});
test.each([{ setupPending: true }, { remote: true }])('Chat is readable but cannot submit with unavailable local setup %j', async props => {
  native(); render(<Example {...props} />); await chat(); compose();
  fireEvent.keyDown(screen.getByRole('textbox', { name: 'Message' }), { key: 'Enter' }); expect(sent).toEqual([]);
  expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(true);
});
test('while a pending save holds a document switch Chat remains bound to the displayed agent', async () => {
  let finish!: (value: unknown) => void; let content = '';
  native(value => { content = value; return new Promise(resolve => { finish = resolve; }); });
  const view = render(<Example />);
  fireEvent.change(await screen.findByRole('textbox', { name: 'Instructions' }), { target: { value: 'Pending switch' } });
  await chat(); compose(); await screen.findByText('Saving…');
  const other = 'a7f34ba2-b9bd-4b55-b69d-f0a84a1bf528'; view.rerender(<Example documentId={other} />);
  expect(screen.getByRole('textbox', { name: 'Message' }).id).toBe(`message-${id}`);
  expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(true); expect(sent).toEqual([]);
  await act(async () => finish({ ...pair, agent: content }));
});
