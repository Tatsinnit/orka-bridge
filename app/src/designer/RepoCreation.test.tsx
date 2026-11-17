import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import { agentCall, SAMPLE_DOCUMENT_ID } from './agent-api';
import Example from './Example';
import NewAgent from './NewAgent';

const id = '12345678-1234-4234-8234-123456789abc';
const selection = { id: '22345678-1234-4234-8234-123456789abc', path: 'C:\\work\\my-agents' };
const pair = { documentId: SAMPLE_DOCUMENT_ID, name: 'web-reader', agent: '---\nname: web-reader\n---\nRead a page.', tests: '# Checks', storage: 'repo' };
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
function native(reply: (command: string, args: Record<string, unknown>) => unknown) {
  vi.stubGlobal('isTauri', true);
  mockIPC((command, value) => command === 'agent_creation_root' ? '/home/creator/Orka/agents' : reply(command, value as Record<string, unknown>));
}
function form() {
  const created = vi.fn(async () => {});
  const registered = vi.fn();
  const view = render(<NewAgent active onCreated={created} onRegistered={registered} onCancel={() => {}} />);
  return { ...view, created, registered };
}

test('repo choice requires native selection, previews both files and submits only its token', async () => {
  const calls: string[] = [];
  native((command, args) => {
    calls.push(command);
    if (command === 'agent_pick_repo') { expect(args).toEqual({}); return selection; }
    if (command === 'agent_create') { expect(args).toEqual({ name: 'web-reader', repoSelection: selection.id }); return id; }
    throw Error(command);
  });
  const user = userEvent.setup(); const { created, registered } = form();
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'web-reader');
  await user.click(screen.getByRole('radio', { name: 'Your own repo' }));
  expect(screen.getByRole('button', { name: 'Create' }).matches(':disabled')).toBe(true);
  expect(screen.queryByText(/web-reader are reserved/)).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Choose repo' }));
  expect(await screen.findByText('C:\\work\\my-agents\\web-reader\\')).toBeTruthy();
  expect(screen.getByText('web-reader.agent.md')).toBeTruthy();
  expect(screen.getByText('web-reader.agent.test.md')).toBeTruthy();
  expect(screen.getByText('Files save automatically. You manage commits in this repo.')).toBeTruthy();
  expect(screen.queryByText(/local Git history/)).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Create' }));
  await waitFor(() => expect(created).toHaveBeenCalledWith(id));
  expect(registered).toHaveBeenCalledOnce();
  expect(calls).toEqual(['agent_pick_repo', 'agent_create']);
});

test('cancelled folder picker retains the name and previous selected repo', async () => {
  let picks = 0;
  native(command => command === 'agent_pick_repo' ? ++picks === 1 ? selection : null : undefined);
  const user = userEvent.setup(); form();
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'helper');
  await user.click(screen.getByRole('radio', { name: 'Your own repo' }));
  await user.click(screen.getByRole('button', { name: 'Choose repo' }));
  await screen.findByText('C:\\work\\my-agents\\helper\\');
  await user.click(screen.getByRole('button', { name: 'Change repo' }));
  expect((screen.getByRole('textbox', { name: 'Name' }) as HTMLInputElement).value).toBe('helper');
  expect(screen.getByText('C:\\work\\my-agents\\helper\\')).toBeTruthy();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(screen.getByRole('button', { name: 'Create' }).matches(':disabled')).toBe(false);
});

test('invalid checkout or malformed picker reply leaves creation unavailable with a safe retry', async () => {
  let picks = 0;
  native(command => {
    if (command === 'agent_pick_repo') { if (++picks === 1) throw Error('/private/repo/config'); return { ...selection, id: '/different/path' }; }
    throw Error('Must not create without authority');
  });
  const user = userEvent.setup(); form();
  await user.click(screen.getByRole('radio', { name: 'Your own repo' }));
  for (let attempt = 0; attempt < 2; attempt++) {
    await user.click(screen.getByRole('button', { name: 'Choose repo' }));
    expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringMatching(/working checkout/i));
    expect(screen.getByRole('button', { name: 'Create' }).matches(':disabled')).toBe(true);
    expect(screen.queryByText('/private/repo/config')).toBeNull();
  }
});

test('picker admission disables duplicate requests and ignores replies after leaving the form', async () => {
  let finish!: (value: unknown) => void; let picks = 0;
  native(command => { if (command === 'agent_pick_repo') { picks++; return new Promise(resolve => { finish = resolve; }); } });
  const user = userEvent.setup(); const view = form();
  await user.click(screen.getByRole('radio', { name: 'Your own repo' }));
  await user.click(screen.getByRole('button', { name: 'Choose repo' }));
  expect(screen.getByRole('button', { name: 'Choosing…' }).matches(':disabled')).toBe(true);
  expect(screen.getByRole('radio', { name: "Bridge's folder" }).matches(':disabled')).toBe(true);
  expect(screen.getByRole('button', { name: 'Create' }).matches(':disabled')).toBe(true);
  view.unmount();
  await act(async () => finish(selection));
  expect(picks).toBe(1); expect(view.created).not.toHaveBeenCalled(); expect(view.registered).not.toHaveBeenCalled();
});

test('switching back to Bridge drops repo authority from creation and restores history copy', async () => {
  native((command, args) => {
    if (command === 'agent_pick_repo') return selection;
    if (command === 'agent_create') { expect(args).toEqual({ name: 'helper' }); return id; }
  });
  const user = userEvent.setup(); const { created } = form();
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'helper');
  await user.click(screen.getByRole('radio', { name: 'Your own repo' }));
  await user.click(screen.getByRole('button', { name: 'Choose repo' }));
  await screen.findByText('C:\\work\\my-agents\\helper\\');
  await user.click(screen.getByRole('radio', { name: "Bridge's folder" }));
  expect(await screen.findByText('/home/creator/Orka/agents/helper/')).toBeTruthy();
  expect(screen.getByText('Saved on this computer with local Git history. Nothing is uploaded.')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Create' }));
  await waitFor(() => expect(created).toHaveBeenCalledWith(id));
});

test('repo create failure keeps the chosen checkout and name for retry', async () => {
  let attempts = 0;
  native((command, args) => {
    if (command === 'agent_pick_repo') return selection;
    if (command === 'agent_create') { expect(args.repoSelection).toBe(selection.id); if (++attempts === 1) throw Error('collision'); return id; }
  });
  const user = userEvent.setup(); const { created } = form();
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'helper');
  await user.click(screen.getByRole('radio', { name: 'Your own repo' }));
  await user.click(screen.getByRole('button', { name: 'Choose repo' }));
  await user.click(screen.getByRole('button', { name: 'Create' }));
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringMatching(/could not be created/i));
  expect((screen.getByRole('textbox', { name: 'Name' }) as HTMLInputElement).value).toBe('helper');
  expect(screen.getByText('C:\\work\\my-agents\\helper\\')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Create' }));
  await waitFor(() => expect(created).toHaveBeenCalledWith(id));
});

test('same-named agents show their distinct locations and select by document ID', async () => {
  const opened: unknown[] = [];
  native((command, args) => {
    if (command === 'agents_list') return [
      { id: SAMPLE_DOCUMENT_ID, name: 'web-reader', path: '/home/creator/Orka/agents/web-reader/web-reader.agent.md', available: true },
      { id, name: 'web-reader', path: '/repos/team/web-reader/web-reader.agent.md', available: true },
    ];
    if (command === 'agent_open') { opened.push(args.documentId); return { ...pair, documentId: args.documentId, storage: args.documentId === id ? 'repo' : 'bridge' }; }
    if (command === 'agent_select') return null;
    if (command === 'agent_run_status') return { documentId: args.documentId, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    if (command === 'send_status') return { documentId: args.documentId, view: null, busy: null };
    if (command === 'send_history') return [];
  });
  const user = userEvent.setup(); render(<Example setupPending />);
  expect(await screen.findByRole('option', { name: 'web-reader (/repos/team/web-reader/web-reader.agent.md)' })).toBeTruthy();
  expect(screen.getByRole('option', { name: 'web-reader (/home/creator/Orka/agents/web-reader/web-reader.agent.md)' })).toBeTruthy();
  await user.selectOptions(screen.getByRole('combobox', { name: 'Agent' }), id);
  await screen.findByText('Files save automatically. You manage commits in this repo.');
  expect(opened.at(-1)).toBe(id);
});

test('queued repo autosaves use the last confirmed agent fingerprint, including the preceding acknowledgement', async () => {
  const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
  const next = { ...source, agent: 'c'.repeat(64) };
  const own = { ...pair, documentId: id, source };
  const saves: Record<string, unknown>[] = [];
  let acknowledge!: (value: unknown) => void;
  native((command, args) => {
    if (command === 'agent_open') return own;
    if (command === 'agent_save') {
      saves.push(args);
      if (saves.length === 1) return new Promise(resolve => { acknowledge = resolve; });
      return { ...own, agent: args.content, source: { ...source, agent: 'd'.repeat(64) } };
    }
    if (command === 'agent_run_status') return { documentId: id, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    if (command === 'send_status') return { documentId: id, view: null, busy: null };
    if (command === 'send_history') return [];
  });
  const user = userEvent.setup(); render(<Example documentId={id} />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await user.click(screen.getByRole('button', { name: 'File' }));
  const editor = screen.getByRole('textbox', { name: 'Agent file' });
  const first = `${pair.agent}\nFirst draft`; const second = `${pair.agent}\nSecond draft`;
  fireEvent.change(editor, { target: { value: first } });
  await waitFor(() => expect(saves).toHaveLength(1));
  fireEvent.change(editor, { target: { value: second } });
  await act(async () => acknowledge({ ...own, agent: first, source: next }));
  await waitFor(() => expect(saves).toHaveLength(2));
  expect(saves[0]).toEqual({ documentId: id, content: first, expectedSource: source });
  expect(saves[1]).toEqual({ documentId: id, content: second, expectedSource: next });
  expect(editor).toHaveProperty('value', second);
});

test('repo conflict retains the draft and its expected fingerprint on retry', async () => {
  const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
  const saves: Record<string, unknown>[] = [];
  native((command, args) => {
    if (command === 'agent_open') return { ...pair, documentId: id, source };
    if (command === 'agent_save') { saves.push(args); throw Error('The agent file changed on disk.'); }
    if (command === 'agent_run_status') return { documentId: id, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    if (command === 'send_status') return { documentId: id, view: null, busy: null };
    if (command === 'send_history') return [];
  });
  const user = userEvent.setup(); render(<Example documentId={id} />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await user.click(screen.getByRole('button', { name: 'File' }));
  const editor = screen.getByRole('textbox', { name: 'Agent file' });
  const draft = `${pair.agent}\nKeep this draft`;
  fireEvent.change(editor, { target: { value: draft } });
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining('changed on disk'));
  expect(editor).toHaveProperty('value', draft);
  await user.click(screen.getByRole('button', { name: 'Try saving again' }));
  await waitFor(() => expect(saves).toHaveLength(2));
  expect(saves).toEqual(Array.from({ length: 2 }, () => ({ documentId: id, content: draft, expectedSource: source })));
  expect(editor).toHaveProperty('value', draft);
});

test('native storage metadata is preserved and unknown values are rejected', async () => {
  native(() => pair);
  expect((await agentCall('agent_open')).storage).toBe('repo');
  native(() => ({ ...pair, storage: 'other' }));
  await expect(agentCall('agent_open')).rejects.toThrow('could not be opened');
});

test.each(['repo', 'bridge', undefined] as const)('a reopened document describes only its known storage (%s)', async storage => {
  const withoutStorage = { documentId: pair.documentId, name: pair.name, agent: pair.agent, tests: pair.tests };
  native((command, args) => {
    if (command === 'agent_open') return storage ? { ...pair, storage } : withoutStorage;
    if (command === 'agent_run_status') return { documentId: args.documentId, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    if (command === 'send_status') return { documentId: args.documentId, view: null, busy: null };
    if (command === 'send_history') return [];
  });
  render(<Example setupPending />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  expect(!!screen.queryByText('Files save automatically. You manage commits in this repo.')).toBe(storage === 'repo');
  expect(!!screen.queryByText(/local Git history/)).toBe(storage === 'bridge');
});
