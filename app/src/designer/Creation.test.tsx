import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import { useState } from 'react';
import Example from './Example';
import App from '../App';
import { SAMPLE_DOCUMENT_ID } from './agent-api';

const id = '12345678-1234-4234-8234-123456789abc';
const pair = { documentId: SAMPLE_DOCUMENT_ID, name: 'web-reader', agent: '---\nname: web-reader\nmodel: shipped\n---\nRead a page.', tests: '# Checks', path: '/home/creator/Orka/agents/web-reader/web-reader.agent.md' };
const draftTests = '---\nschemaVersion: 1\n---\n\n# helper tests\n\n```yaml agent-test\nid: first-test\ninput: ""\nassertions: []\n```\n';
const other = { ...pair, documentId: id, name: 'helper', agent: '---\nname: "helper"\nmodel: "gpt-5.6-terra"\nkmx:\n  schemaVersion: 1\n  runtime: orka\n---\n\n', tests: draftTests };
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });
function native(extra?: (command: string, args: Record<string, unknown>) => unknown) {
  vi.stubGlobal('isTauri', true);
  mockIPC((command, value) => {
    const args = value as Record<string, unknown>;
    const result = extra?.(command, args);
    if (result !== undefined) return result;
    if (command === 'agent_creation_root') return '/home/creator/Orka/agents';
    if (command === 'agent_selected') return SAMPLE_DOCUMENT_ID;
    if (command === 'agents_list') return [{ id: SAMPLE_DOCUMENT_ID, name: 'web-reader', path: pair.path, available: true }, { id, name: 'helper', path: '/home/creator/Orka/agents/helper/helper.agent.md', available: true }];
    if (command === 'agent_open') return args.documentId === id ? other : pair;
    if (command === 'agent_select') return null;
    if (command === 'agent_run_status') return { documentId: args.documentId, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    if (command === 'send_status') return { documentId: args.documentId, view: null, busy: null };
    if (command === 'send_history') return [];
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'starting', model: 'locked' };
    if (command === 'setup_example') return { agent: pair.agent, tests: pair.tests };
    throw Error(`Unexpected ${command}`);
  });
}

test('creates during setup, previews the native location and both files, and opens without running', async () => {
  const calls: string[] = [];
  native((command, args) => { calls.push(command); if (command === 'agent_create') { expect(args).toEqual({ name: 'helper' }); return id; } });
  const user = userEvent.setup(); render(<Example setupPending />);
  const entry = await screen.findByRole('button', { name: 'New agent' });
  expect(entry.classList.contains('button-secondary')).toBe(true);
  await user.click(entry);
  expect(screen.getByRole('textbox', { name: 'Name' }).classList.contains('field')).toBe(true);
  expect(screen.getByRole('button', { name: 'Create' }).classList.contains('button-primary')).toBe(true);
  expect(screen.getByRole('button', { name: 'Cancel' }).classList.contains('button-secondary')).toBe(true);
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'helper');
  expect(await screen.findByText('/home/creator/Orka/agents/helper/')).toBeTruthy();
  expect(screen.getByText('helper.agent.md')).toBeTruthy();
  expect(screen.getByText('helper.agent.test.md')).toBeTruthy();
  expect((screen.getByRole('radio', { name: "Bridge's folder" }) as HTMLInputElement).checked).toBe(true);
  expect((screen.getByRole('radio', { name: 'Your own repo' }) as HTMLInputElement).checked).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Create' }));
  expect(await screen.findByRole('heading', { name: 'helper', level: 1 })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Run tests' }).matches(':disabled')).toBe(true);
  expect(calls).not.toContain('agent_run'); expect(calls).not.toContain('workspace_catalog');
});

test.each(['Chat', 'File'])('creation from %s opens empty Instructions with an incomplete test, never a format error', async previousView => {
  const calls: string[] = [];
  native((command, args) => {
    calls.push(command);
    if (command === 'agent_create') return id;
    if (command === 'agent_chat_status') return { documentId: args.documentId, state: 'ready', busy: false, canSend: true, versions: [], error: null };
  });
  const user = userEvent.setup(); render(<Example />);
  await user.click(await screen.findByRole('button', { name: previousView }));
  await user.click(screen.getByRole('button', { name: 'New agent' }));
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'helper');
  await user.click(screen.getByRole('button', { name: 'Create' }));
  await screen.findByRole('heading', { name: 'helper', level: 1 });
  expect(screen.getByRole('button', { name: 'Agent' }).getAttribute('aria-pressed')).toBe('true');
  expect((screen.getByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement).value.trim()).toBe('');
  expect(screen.getByText(/test is incomplete/i)).toBeTruthy();
  expect(screen.queryByText(/test list could not be read/i)).toBeNull();
  expect(screen.getByRole('button', { name: 'Run tests' }).matches(':disabled')).toBe(true);
  expect(screen.getByRole('button', { name: 'Send to a cluster' }).matches(':disabled')).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Chat' }));
  expect(screen.getByText(/write instructions in Agent view before chatting/i)).toBeTruthy();
  await screen.findByText('Chat is unavailable.');
  await user.type(screen.getByRole('textbox', { name: 'Message' }), 'Hello{Enter}');
  expect(calls).not.toContain('agent_run');
  expect(calls).not.toContain('agent_chat_send');
});

test('an incomplete draft reloads a completed test file and keeps instructions on a read failure', async () => {
  let disk = { ...other, agent: other.agent + 'Answer briefly.\n' }; let fail = false;
  native(command => { if (command === 'agent_open') { if (fail) throw Error('/private/read/failure'); return disk; } });
  const user = userEvent.setup(); render(<Example documentId={id} />);
  await screen.findByText(/test is incomplete/i);
  const editor = screen.getByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement;
  fail = true; await user.click(screen.getByRole('button', { name: 'Reload saved files' }));
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining('saved files could not be read'));
  expect(editor.value).toBe('Answer briefly.\n');
  expect(screen.queryByText('/private/read/failure')).toBeNull();
  fail = false; disk = { ...disk, tests: '---\nschemaVersion: 1\n---\n\n```yaml agent-test\nid: first-test\ntitle: Checks the greeting\ninput: Hi\nassertions:\n  - id: greeting\n    type: contains\n    value: Hi\n```\n' };
  await user.click(screen.getByRole('button', { name: 'Reload saved files' }));
  expect(await screen.findByText('Checks the greeting')).toBeTruthy();
  expect(screen.queryByText(/test is incomplete/i)).toBeNull();
  expect((screen.getByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement).value).toBe('Answer briefly.\n');
});
test('an Overview Run request is consumed when its test is still incomplete', async () => {
  native(); const handled = vi.fn();
  render(<Example documentId={id} actionRequest={{ id: 99, documentId: id, action: 'run' }} onActionHandled={handled} />);
  await screen.findByText(/test is incomplete/i);
  await waitFor(() => expect(handled.mock.calls).toEqual([[99]]));
});
test('draft saved-file reload is disabled while instructions have unsaved edits', async () => {
  let opens = 0;
  native(command => { if (command === 'agent_open') { opens++; return other; } });
  const user = userEvent.setup(); render(<Example documentId={id} />);
  const editor = await screen.findByRole('textbox', { name: 'Instructions' });
  fireEvent.change(editor, { target: { value: 'Keep my draft.' } });
  const reload = screen.getByRole('button', { name: 'Reload saved files' });
  expect(reload.matches(':disabled')).toBe(true);
  await user.click(reload); expect(opens).toBe(1);
  expect((editor as HTMLTextAreaElement).value).toBe('Keep my draft.');
});

test('naming details are available on request without hiding either storage choice', async () => {
  native(); const user = userEvent.setup(); render(<Example />);
  await user.click(await screen.findByRole('button', { name: 'New agent' }));
  const naming = screen.getByText('Naming details').closest('details')!;
  expect(naming.open).toBe(false);
  expect(screen.getAllByRole('radio')).toHaveLength(2);
  await user.click(screen.getByText('Naming details'));
  expect(naming.open).toBe(true);
});

test('cancel preserves the mounted draft and creates nothing', async () => {
  let creates = 0;
  native(command => { if (command === 'agent_create') creates++; });
  const user = userEvent.setup(); render(<Example setupPending />);
  const editor = await screen.findByRole('textbox', { name: 'Instructions' });
  await user.click(screen.getByRole('button', { name: 'New agent' }));
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'helper');
  await user.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(screen.getByRole('textbox', { name: 'Instructions' })).toBe(editor);
  expect(creates).toBe(0);
});

test('creation failure keeps the name, previous document and safe error for retry', async () => {
  native(command => { if (command === 'agent_create') throw Error('/private/native/error'); });
  const user = userEvent.setup(); render(<Example />);
  await user.click(await screen.findByRole('button', { name: 'New agent' }));
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'helper');
  await user.click(screen.getByRole('button', { name: 'Create' }));
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringMatching(/could not be created/i));
  expect((screen.getByRole('textbox', { name: 'Name' }) as HTMLInputElement).value).toBe('helper');
  expect(screen.queryByText('/private/native/error')).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(screen.getByRole('heading', { name: 'web-reader', level: 1 })).toBeTruthy();
});

test('New agent waits for the current save and keeps failed draft visible until retry', async () => {
  let reject!: (reason: Error) => void; let saved = false;
  native((command, args) => {
    if (command === 'agent_save') return saved ? { ...pair, agent: args.content } : new Promise((_resolve, fail) => { reject = fail; });
  });
  const user = userEvent.setup(); render(<Example />);
  const editor = await screen.findByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement;
  fireEvent.change(editor, { target: { value: 'Keep my work.' } });
  await user.click(screen.getByRole('button', { name: 'New agent' }));
  await waitFor(() => expect(reject).toBeDefined());
  expect(screen.queryByRole('textbox', { name: 'Name' })).toBeNull();
  await act(async () => reject(Error('disk full')));
  expect(await screen.findByText('Not saved')).toBeTruthy();
  expect(editor.value).toBe('Keep my work.');
  saved = true;
  await user.click(screen.getByRole('button', { name: 'Try saving again' }));
  expect(await screen.findByRole('textbox', { name: 'Name' })).toBeTruthy();
});

test('selector persists identity and opens a different pair', async () => {
  let selected: unknown;
  native((command, args) => { if (command === 'agent_select') { selected = args.documentId; return null; } });
  const user = userEvent.setup(); render(<Example />);
  const selector = await screen.findByRole('combobox', { name: 'Agent' });
  expect(selector.classList.contains('field')).toBe(true);
  await user.selectOptions(selector, id);
  expect(await screen.findByRole('heading', { name: 'helper', level: 1 })).toBeTruthy();
  expect(selected).toBe(id);
});

test('first Designer open restores even an unavailable selection without opening the sample', async () => {
  const opened: unknown[] = [];
  native((command, args) => {
    if (command === 'agent_selected') return id;
    if (command === 'agent_open') { opened.push(args.documentId); throw Error('moved'); }
    if (command === 'agents_list') return [{ id, name: 'helper', path: null, available: false }];
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect(await screen.findByText(/could not be opened/)).toBeTruthy();
  expect(opened).toEqual([id]);
  expect(screen.getByRole('option', { name: 'helper (unavailable)' })).toBeTruthy();
});

test('creation uses an already loaded catalog default without inheriting the sample model', async () => {
  native((command, args) => {
    if (command === 'workspace_catalog') return { defaultModel: 'workspace-model', models: { state: 'ready', items: [{ id: 'workspace-model' }] }, tools: { state: 'ready', items: [] } };
    if (command === 'agent_create') { expect(args).toEqual({ name: 'helper', model: 'workspace-model' }); return id; }
  });
  const user = userEvent.setup(); render(<Example catalogKey={1} />);
  await screen.findByRole('option', { name: 'Workspace default: workspace-model' });
  await user.click(screen.getByRole('button', { name: 'New agent' }));
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'helper');
  await user.click(screen.getByRole('button', { name: 'Create' }));
  expect(await screen.findByRole('heading', { name: 'helper', level: 1 })).toBeTruthy();
});

test('creation waits for personality digest and saves its text before hiding the editor', async () => {
  let finish!: (value: ArrayBuffer) => void; const writes: string[] = [];
  vi.stubGlobal('crypto', { subtle: { digest: () => new Promise(resolve => { finish = resolve; }) } });
  const agent = '---\nname: web-reader\nmodel: shipped\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  soul:\n    inline: Old personality\n    digest: sha256:old\n---\nRead a page.';
  native((command, args) => {
    if (command === 'agent_open') return { ...pair, agent };
    if (command === 'agent_save') { writes.push(String(args.content)); return { ...pair, agent: args.content }; }
  });
  const user = userEvent.setup(); render(<Example />);
  await user.click(await screen.findByText('More settings'));
  fireEvent.change(screen.getByRole('textbox', { name: 'Personality' }), { target: { value: 'Keep my personality' } });
  await user.click(screen.getByRole('button', { name: 'New agent' }));
  expect(screen.queryByRole('textbox', { name: 'Name' })).toBeNull();
  await act(async () => finish(new Uint8Array(32).buffer));
  expect(await screen.findByRole('textbox', { name: 'Name' })).toBeTruthy();
  expect(writes).toHaveLength(1); expect(writes[0]).toContain('Keep my personality');
});

test('the first Overview open is remembered even before Designer has mounted', async () => {
  let selected: unknown;
  native((command, args) => {
    if (command === 'overview_files') return { state: 'complete', runActive: false, files: [{ documentId: id, name: 'helper', state: 'readable', source: null, summary: null, reason: null }] };
    if (command === 'agent_select') { selected = args.documentId; return null; }
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open helper' }));
  await screen.findByRole('heading', { name: 'helper', level: 1 });
  await waitFor(() => expect(selected).toBe(id));
});

test('a newer switch request wins over an older selection reply', async () => {
  let finish!: (value: null) => void; const selected: unknown[] = [];
  native((command, args) => {
    if (command === 'agent_select') { selected.push(args.documentId); if (args.documentId === id) return new Promise(resolve => { finish = resolve; }); return null; }
  });
  function Switcher() {
    const [current, setCurrent] = useState(SAMPLE_DOCUMENT_ID);
    return <><button onClick={() => setCurrent(id)}>Choose helper</button><button onClick={() => setCurrent(SAMPLE_DOCUMENT_ID)}>Choose sample</button><Example documentId={current} onDocument={setCurrent} /></>;
  }
  render(<Switcher />);
  await screen.findByRole('heading', { name: 'web-reader', level: 1 });
  fireEvent.click(screen.getByRole('button', { name: 'Choose helper' }));
  await waitFor(() => expect(finish).toBeDefined());
  fireEvent.click(screen.getByRole('button', { name: 'Choose sample' }));
  await act(async () => finish(null));
  await waitFor(() => expect(selected.at(-1)).toBe(SAMPLE_DOCUMENT_ID));
  expect(screen.getByRole('heading', { name: 'web-reader', level: 1 })).toBeTruthy();
});

test('edits made during selection are saved before the reply can clear the draft', async () => {
  let finish!: (value: null) => void; let selects = 0; const writes: string[] = [];
  native((command, args) => {
    if (command === 'agent_select' && selects++ === 0) return new Promise(resolve => { finish = resolve; });
    if (command === 'agent_save') { writes.push(String(args.content)); return { ...pair, agent: args.content }; }
  });
  const view = render(<Example />);
  const editor = await screen.findByRole('textbox', { name: 'Instructions' });
  view.rerender(<Example documentId={id} />);
  await waitFor(() => expect(finish).toBeDefined());
  fireEvent.change(editor, { target: { value: 'Save this late edit' } });
  await act(async () => finish(null));
  await screen.findByRole('heading', { name: 'helper', level: 1 });
  expect(writes).toHaveLength(1); expect(writes[0]).toContain('Save this late edit');
});

test('a save already in flight finishes before selection can reset its source', async () => {
  let finishSelect!: (value: null) => void; let finishSave!: (value: unknown) => void;
  let selects = 0; const writes: string[] = [];
  native((command, args) => {
    if (command === 'agent_select' && selects++ === 0) return new Promise(resolve => { finishSelect = resolve; });
    if (command === 'agent_save') { writes.push(String(args.content)); return new Promise(resolve => { finishSave = resolve; }); }
  });
  const view = render(<Example />);
  const editor = await screen.findByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement;
  view.rerender(<Example documentId={id} />);
  await waitFor(() => expect(finishSelect).toBeDefined());
  fireEvent.change(editor, { target: { value: 'Save in flight' } });
  await waitFor(() => expect(finishSave).toBeDefined());
  await act(async () => finishSelect(null));
  expect(editor.value).toBe('Save in flight');
  expect(screen.queryByRole('heading', { name: 'helper', level: 1 })).toBeNull();
  await act(async () => finishSave({ ...pair, agent: writes[0] }));
  await screen.findByRole('heading', { name: 'helper', level: 1 });
  expect(writes).toHaveLength(1); expect(writes[0]).toContain('Save in flight');
});

test.each([false, true])('Overview open exits the form and wins over an in-flight create (%s)', async pending => {
  let finish!: (value: string) => void; let created = false;
  native(command => {
    if (command === 'agents_list') return [{ id: SAMPLE_DOCUMENT_ID, name: 'web-reader', path: pair.path, available: true }, ...(created ? [{ id, name: 'helper', path: '/home/creator/Orka/agents/helper/helper.agent.md', available: true }] : [])];
    if (command === 'overview_files') return { state: 'complete', runActive: false, files: [{ documentId: SAMPLE_DOCUMENT_ID, name: 'web-reader', state: 'readable', source: null, summary: null, reason: null }] };
    if (command === 'agent_create') return new Promise(resolve => { finish = resolve; });
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  await user.click(await screen.findByRole('button', { name: 'New agent' }));
  if (pending) {
    await user.type(screen.getByRole('textbox', { name: 'Name' }), 'helper');
    await user.click(screen.getByRole('button', { name: 'Create' }));
    await waitFor(() => expect(finish).toBeDefined());
  }
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open web-reader' }));
  if (pending) await act(async () => { created = true; finish(id); });
  expect(await screen.findByRole('heading', { name: 'web-reader', level: 1 })).toBeTruthy();
  if (pending) await screen.findByRole('option', { name: 'helper' });
  expect(screen.queryByRole('textbox', { name: 'Name' })).toBeNull();
  expect(screen.queryByRole('heading', { name: 'helper', level: 1 })).toBeNull();
});

test('a failed restored agent can switch persistently and learn of a creation completed after remount', async () => {
  const third = '22345678-1234-4234-8234-123456789abc';
  let finish!: (value: string) => void; let created = false; let selected: unknown;
  native((command, args) => {
    if (command === 'agent_open' && args.documentId === SAMPLE_DOCUMENT_ID) throw Error('moved');
    if (command === 'agent_select') { selected = args.documentId; return null; }
    if (command === 'agent_create') return new Promise(resolve => { finish = resolve; });
    if (command === 'agents_list') return [{ id, name: 'helper', path: '/home/creator/Orka/agents/helper/helper.agent.md', available: true }, ...(created ? [{ id: third, name: 'third', path: '/home/creator/Orka/agents/third/third.agent.md', available: true }] : [])];
    if (command === 'overview_files') return { state: 'complete', runActive: false, files: [{ documentId: id, name: 'helper', state: 'readable', source: null, summary: null, reason: null }] };
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  await screen.findByText(/folder could not be opened/);
  await user.click(screen.getByRole('button', { name: 'New agent' }));
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'third');
  await user.click(screen.getByRole('button', { name: 'Create' }));
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open helper' }));
  await screen.findByRole('heading', { name: 'helper', level: 1 });
  await waitFor(() => expect(selected).toBe(id));
  await act(async () => { created = true; finish(third); });
  await screen.findByRole('option', { name: 'third' });
  expect(screen.getByRole('heading', { name: 'helper', level: 1 })).toBeTruthy();
});

test('a selector choice replaces New agent queued behind a failed save', async () => {
  let fail = true;
  native((command, args) => { if (command === 'agent_save') { if (fail) throw Error('disk full'); return { ...pair, agent: args.content }; } });
  const user = userEvent.setup(); render(<Example />);
  fireEvent.change(await screen.findByRole('textbox', { name: 'Instructions' }), { target: { value: 'Keep draft' } });
  await user.click(screen.getByRole('button', { name: 'New agent' }));
  await screen.findByText('Not saved');
  await user.selectOptions(screen.getByRole('combobox', { name: 'Agent' }), id);
  fail = false; await user.click(screen.getByRole('button', { name: 'Try saving again' }));
  expect(await screen.findByRole('heading', { name: 'helper', level: 1 })).toBeTruthy();
  expect(screen.queryByRole('textbox', { name: 'Name' })).toBeNull();
});

test('a failed personality digest retains earlier instructions until the personality is corrected', async () => {
  let reject!: (reason: Error) => void; const writes: string[] = [];
  const digest = vi.fn().mockImplementationOnce(() => new Promise((_resolve, fail) => { reject = fail; })).mockResolvedValue(new Uint8Array(32).buffer);
  vi.stubGlobal('crypto', { subtle: { digest } });
  const agent = '---\nname: web-reader\nmodel: shipped\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  soul:\n    inline: Old personality\n    digest: sha256:old\n---\nRead a page.';
  native((command, args) => {
    if (command === 'agent_open') return { ...pair, agent };
    if (command === 'agent_save') { writes.push(String(args.content)); return { ...pair, agent: args.content }; }
  });
  const user = userEvent.setup(); render(<Example />);
  fireEvent.change(await screen.findByRole('textbox', { name: 'Instructions' }), { target: { value: 'Instructions that must save' } });
  await user.click(screen.getByText('More settings'));
  fireEvent.change(screen.getByRole('textbox', { name: 'Personality' }), { target: { value: 'Pending personality' } });
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 600)); });
  expect(writes).toHaveLength(0);
  await act(async () => reject(Error('digest failed')));
  await screen.findByRole('alert');
  expect(writes).toHaveLength(0);
  expect((screen.getByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement).value).toBe('Instructions that must save');
  await user.click(screen.getByRole('button', { name: 'New agent' }));
  expect(screen.queryByRole('textbox', { name: 'Name' })).toBeNull();
  fireEvent.change(screen.getByRole('textbox', { name: 'Personality' }), { target: { value: 'Corrected personality' } });
  await screen.findByRole('textbox', { name: 'Name' });
  expect(writes).toHaveLength(1);
  expect(writes[0]).toContain('Instructions that must save');
  expect(writes[0]).toContain('Corrected personality');
});

test.each(['', 'Upper', 'helper'])('Enter validates the name before native creation (%s)', async name => {
  const created: unknown[] = [];
  native((command, args) => { if (command === 'agent_create') { created.push(args.name); return id; } });
  const user = userEvent.setup(); render(<Example />);
  await user.click(await screen.findByRole('button', { name: 'New agent' }));
  const input = screen.getByRole('textbox', { name: 'Name' });
  if (name) await user.type(input, name);
  await user.keyboard('{Enter}');
  if (name === 'helper') {
    await screen.findByRole('heading', { name: 'helper', level: 1 });
    expect(created).toEqual(['helper']);
  } else { expect(created).toEqual([]); expect(screen.getByRole('textbox', { name: 'Name' })).toBe(input); }
});

test('a remounted creation does not replay the previously consumed Open request', async () => {
  const selected: unknown[] = [];
  native((command, args) => { if (command === 'agent_select') { selected.push(args.documentId); return null; } });
  render(<Example openDocumentRequest={7} newAgentRequest={1} />);
  await screen.findByRole('textbox', { name: 'Name' });
  await waitFor(() => expect(screen.queryByText('Opening your agent…')).toBeNull());
  expect(selected).toEqual([]);
});

test('selection completion cannot update the parent after its editor was unmounted', async () => {
  let finish!: (value: null) => void; const changed = vi.fn();
  native(command => { if (command === 'agent_select') return new Promise(resolve => { finish = resolve; }); });
  const view = render(<Example onDocument={changed} />);
  await screen.findByRole('heading', { name: 'web-reader', level: 1 });
  view.rerender(<Example documentId={id} onDocument={changed} />);
  await waitFor(() => expect(finish).toBeDefined());
  view.unmount(); await act(async () => finish(null));
  expect(changed).not.toHaveBeenCalled();
});

test.each([false, true])('ordinary tabs preserve creation after a restored source failure (%s)', async pending => {
  let missing = true; let finish!: (value: string) => void;
  native((command, args) => {
    if (command === 'agent_open' && args.documentId === SAMPLE_DOCUMENT_ID && missing) throw Error('moved');
    if (command === 'agent_create') return new Promise(resolve => { finish = resolve; });
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  await screen.findByText(/folder could not be opened/);
  await user.click(screen.getByRole('button', { name: 'New agent' }));
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'helper');
  if (pending) { await user.click(screen.getByRole('button', { name: 'Create' })); await waitFor(() => expect(finish).toBeDefined()); }
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect((screen.getByRole('textbox', { name: 'Name' }) as HTMLInputElement).value).toBe('helper');
  if (pending) { await act(async () => finish(id)); await screen.findByRole('heading', { name: 'helper', level: 1 }); }
  else {
    await user.click(screen.getByRole('button', { name: 'Cancel' })); missing = false;
    await user.click(screen.getByRole('button', { name: 'Try opening again' }));
    await screen.findByRole('heading', { name: 'web-reader', level: 1 });
  }
});

test('Overview offers the same creation form without starting setup', async () => {
  localStorage.setItem('bridge.welcomeSeen.v1', '1'); native();
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  const entry = screen.getByRole('button', { name: 'New agent' });
  expect(entry.classList.contains('button-primary')).toBe(true);
  await user.click(entry);
  expect(await screen.findByRole('textbox', { name: 'Name' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Designer' }).getAttribute('aria-current')).toBe('page');
});
