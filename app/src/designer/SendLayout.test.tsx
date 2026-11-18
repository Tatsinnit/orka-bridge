import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Example from './Example';
import * as definitions from './test-definitions';
import * as form from './agent-form';

const id = '9b2b93c4-375c-4815-9717-39852a96e411';
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const pair = { documentId: id, name: 'web-reader', agent: '---\nname: web-reader\nmodel: literal-model\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools: []\n---\nRead the page.\n', tests: 'Checks from disk', source };
const target = { id: '00000000-0000-4000-8000-000000000002', context_id: '00000000-0000-4000-8000-000000000001', context_name: 'team-dev', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'ghcr.io/orka:0.9.4' };
const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'team-model', ready: true };
const place = { kind: 'cluster' as const, target, providers: [provider], providerId: provider.id };
const failed = { id: 'reads-the-title', question: 'What is the title?', phase: 'Succeeded', answer: 'No', completedTools: [], assertions: [{ id: 'has', kind: 'contains', expected: 'Title', verdict: 'failed' }], verdict: 'failed' };
const localResult = { documentId: id, busy: null, state: 'complete', stage: 'done', canRun: true, modelConnected: true, source, report: { verdict: 'failed', cases: [failed] }, acknowledged: [], pending: null, error: null };
beforeEach(() => { Object.defineProperty(HTMLDivElement.prototype, 'showPopover', { configurable: true, value() { this.style.display = 'grid'; } }); });
afterEach(() => { cleanup(); clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); Reflect.deleteProperty(HTMLDivElement.prototype, 'showPopover'); });
function native(run: object = localResult, send: object | null = null, opened = pair) {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn((command: string) => {
    if (command === 'agent_open') return opened;
    if (command === 'agent_run_status' || command === 'agent_run') return run;
    if (command === 'send_status') return { documentId: id, view: send, busy: null };
    if (command === 'send_history') return [];
    if (command === 'workspace_catalog' || command === 'cluster_catalog') return { defaultModel: 'literal-model', models: { state: 'ready', items: [{ id: 'literal-model' }] }, tools: { state: 'ready', items: [{ kind: 'builtin', name: 'web_fetch', description: 'Read a page' }] } };
    if (command === 'send_preflight') return { documentId: id, source, targetId: target.id, providerId: provider.id, model: 'literal-model', rows: [{ label: 'AI connection', status: 'allowed', message: 'Ready.' }], canSend: true, action: 'create', canSeparate: false };
    throw Error(`Unexpected ${command}`);
  });
  mockIPC(ipc); return ipc;
}
test('a selected cluster keeps independent local tests and the always-visible Send in one right-hand stack', async () => {
  const ipc = native(); const user = userEvent.setup();
  const { container } = render(<Example remote place={place} />);
  const local = await screen.findByRole('region', { name: 'This computer' });
  const cluster = screen.getByRole('region', { name: 'Team cluster' });
  expect(local.parentElement).toBe(cluster.parentElement);
  expect(local.parentElement?.classList.contains('designer-side')).toBe(true);
  expect(local.compareDocumentPosition(cluster) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  const editor = container.querySelector('.designer-editing')!;
  expect(editor.contains(local) || editor.contains(cluster)).toBe(false);
  expect(editor.compareDocumentPosition(local) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  const sendPanel = within(cluster).getByRole('region', { name: 'Send and cluster tests' });
  expect(sendPanel.hasAttribute('hidden')).toBe(false);
  expect(sendPanel.classList.contains('card')).toBe(true);
  expect(sendPanel.parentElement).toBe(cluster);
  // The earlier local result survives the cluster selection and Run is not gated by it.
  expect(await within(local).findByText('reads-the-title')).toBeTruthy();
  const run = within(local).getByRole('button', { name: 'Run tests' }) as HTMLButtonElement;
  expect(run.disabled).toBe(false);
  await user.click(run);
  await waitFor(() => expect(ipc.mock.calls.some(([command]) => command === 'agent_run')).toBe(true));
  expect(ipc.mock.calls.some(([command]) => command.startsWith('cluster_'))).toBe(false);
});
test('Fix it first moves keyboard focus to the single failing local result, which offers Edit instructions', async () => {
  native(); const user = userEvent.setup();
  render(<Example remote place={place} />);
  const local = await screen.findByRole('region', { name: 'This computer' });
  await within(local).findByText('reads-the-title');
  const send = await screen.findByRole('button', { name: 'Send and run tests' });
  await waitFor(() => expect((send as HTMLButtonElement).disabled).toBe(false));
  await user.click(send);
  await user.click(await screen.findByRole('button', { name: 'Fix it first' }));
  await waitFor(() => expect(document.activeElement?.closest('[data-result-id]')?.getAttribute('data-result-id')).toBe('reads-the-title'));
  expect(local.contains(document.activeElement)).toBe(true);
  await user.click(within(local).getByRole('button', { name: 'Edit instructions' }));
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'Instructions' })));
});
test('Fix it first focuses the visible partial failed result while a local suite is still running', async () => {
  native({ ...localResult, state: 'running', stage: 'observe', canRun: false, busy: { documentId: id, name: 'web-reader' } });
  const user = userEvent.setup(); render(<Example remote place={place} />);
  const send = await screen.findByRole('button', { name: 'Send and run tests' });
  await waitFor(() => expect((send as HTMLButtonElement).disabled).toBe(false));
  await user.click(send);
  await user.click(await screen.findByRole('button', { name: 'Fix it first' }));
  // The test list shows finished results while the suite runs, so the failing one is the focus target.
  await waitFor(() => expect(document.activeElement?.closest('[data-result-id]')?.getAttribute('data-result-id')).toBe('reads-the-title'));
  expect(document.activeElement?.tagName).toBe('SUMMARY');
  expect(screen.queryByRole('dialog')).toBeNull();
});
test('the page-head Send control only brings the Send card into view', async () => {
  const ipc = native(); const user = userEvent.setup();
  render(<Example remote place={place} />);
  const head = await screen.findByRole('button', { name: 'Send to a cluster' });
  await waitFor(() => expect((head as HTMLButtonElement).disabled).toBe(false));
  await user.click(head);
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(ipc.mock.calls.some(([command]) => command === 'send_start')).toBe(false);
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('region', { name: 'Send and cluster tests' })));
});
test('a failed cluster Send offers Fix in Designer and focuses the instructions, not a local test result', async () => {
  native(localResult, { documentId: id, id: '00000000-0000-4000-8000-000000000004', targetId: target.id, providerId: provider.id, targetName: target.context_name, source, model: 'literal-model', agentName: 'generated-agent', agentAction: 'create', state: 'complete', stage: 'done', acknowledged: [{ kind: 'Agent', name: 'generated-agent', uid: 'uid' }], pending: null, report: { verdict: 'failed', cases: [failed] }, error: null, storageError: false, cancellation: null });
  render(<Example remote place={place} />);
  const fix = await screen.findByRole('button', { name: 'Fix in Designer' });
  await userEvent.click(fix);
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'Instructions' })));
});
test.each([
  ['frontmatter-reader', pair.agent.replace('name: web-reader', 'name: frontmatter-reader')],
  ['commented-reader', pair.agent.replace('name: web-reader', 'name: commented-reader # saved name')],
  ['renamed-reader', pair.agent.replace('name: web-reader', 'name: "renamed-reader" # saved name')],
  ['quoted-reader', pair.agent.replace('name: web-reader', "name: 'quoted-reader'")],
  ['renamed-reader', pair.agent.replace('name: web-reader', 'name: renamed-reader\ndescription: &desc Reader')],
  ['reader-folder', pair.agent.replace('name: web-reader\n', '')],
  ['reader-folder', pair.agent.replace('name: web-reader', 'name: 42')],
  ['reader-folder', pair.agent.replace('name: web-reader', 'name: first\nname: second')],
  ['reader-folder', pair.agent.replace('name: web-reader', 'name: [unfinished')],
  ['reader-folder', pair.agent.replace('name: web-reader', 'description: &desc reader\nname: *desc')],
])('Send identifies the saved front-matter name with a folder fallback: %s', async (name, agent) => {
  native(localResult, null, { ...pair, name: 'reader-folder', agent }); render(<Example remote place={place} />);
  const panel = await screen.findByRole('region', { name: 'Send and cluster tests' });
  expect(within(panel).getByRole('region', { name: 'What you send' }).textContent).toContain(`${name} and test files`);
});
test('editing instructions does not reparse unchanged saved tests or the saved agent name', async () => {
  const parse = vi.spyOn(definitions, 'testDefinitions');
  const name = vi.spyOn(form, 'readAgentName'); native();
  render(<Example remote place={place} />);
  const field = await screen.findByRole('textbox', { name: 'Instructions' });
  const reads = parse.mock.calls.length;
  const names = name.mock.calls.length;
  expect(name).toHaveBeenLastCalledWith(pair.agent);
  const user = userEvent.setup(); await user.type(field, ' Edited');
  expect(parse.mock.calls.length).toBe(reads);
  expect(name.mock.calls.length).toBe(names);
});
test('a team Send destination supplies its catalog without disabling local tests', async () => {
  let release!: (value: typeof localResult) => void;
  const ipc = native(new Promise<typeof localResult>(resolve => { release = resolve; }));
  render(<Example remote place={place} catalogKey={1} />);
  const tool = await screen.findByRole('button', { name: 'Add a tool' });
  await waitFor(() => expect((tool as HTMLButtonElement).disabled).toBe(false));
  expect((screen.getByRole('combobox', { name: 'It thinks with' }) as HTMLSelectElement).disabled).toBe(false);
  expect(ipc.mock.calls.some(([command]) => command === 'cluster_catalog')).toBe(true);
  const run = within(screen.getByRole('region', { name: 'This computer' })).getByRole('button', { name: 'Run tests' }) as HTMLButtonElement;
  expect(run.disabled).toBe(true); // Catalog readiness is independent of the pending local snapshot.
  await act(async () => release(localResult));
  await waitFor(() => expect(run.disabled).toBe(false));
});
