import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Example from './Example';
import { agentCall } from './agent-api';
const agent = '---\nname: reader\ndescription: "Reads a page"\nmodel: file-model\nfuture: {keep: yes}\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools:\n    - kind: builtin\n      name: web_fetch\n---\n\nRead a page.\n';
const path = '/home/creator/Orka/agents/web-reader/web-reader.agent.md';
const idle = { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
beforeEach(() => {
  // jsdom has popover hiding but no top-layer show/focus implementation.
  Object.defineProperty(HTMLDivElement.prototype, 'showPopover', { configurable: true, value() { this.style.display = 'grid'; } });
});
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); Reflect.deleteProperty(HTMLDivElement.prototype, 'showPopover'); });
function native(source = agent) {
  const writes: string[] = []; vi.stubGlobal('isTauri', true);
  mockIPC((command, args) => {
    if (command === 'workspace_catalog') return { defaultModel: 'file-model', models: { state: 'ready', items: [{ id: 'file-model' }] }, tools: { state: 'ready', items: [{ kind: 'builtin', name: 'web_fetch', description: 'Read a page' }, { kind: 'builtin', name: 'file_read', description: 'Read a file' }] } };
    if (command === 'agent_open') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: source, tests: 'Checks', path };
    if (command === 'setup_example') return { agent: source, tests: 'Checks' };
    if (command === 'agent_save') { writes.push((args as { content: string }).content); return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: writes.at(-1), tests: 'Checks', path }; }
    return idle;
  });
  return writes;
}
test('explains personality separately from instructions and offers a description hint', async () => {
  native(); const user = userEvent.setup(); render(<Example catalogKey={1} />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await user.click(screen.getByText('More settings'));
  expect(screen.getByRole('textbox', { name: 'Personality' }).getAttribute('aria-describedby')).toBeTruthy();
  expect(screen.getByPlaceholderText(/short summary/i)).toBeTruthy();
  expect(screen.getByText(/Tone and style to keep across tasks/)).toBeTruthy();
});
test('Instructions autosave through the existing writer without changing metadata', async () => {
  const writes = native(); render(<Example />);
  const field = await screen.findByRole('textbox', { name: 'Instructions' });
  expect((field as HTMLTextAreaElement).value).toBe('Read a page.\n');
  fireEvent.change(field, { target: { value: 'New instructions\n' } });
  await waitFor(() => expect(writes).toEqual([agent.replace('Read a page.\n', 'New instructions\n')]));
  await waitFor(() => expect(screen.queryByText('Saving…')).toBeNull());
  expect(screen.queryByText('Saved')).toBeNull();
  expect(writes[0]).toContain('---\n\nNew instructions\n');
});
test('new leading blank lines remain visible and removable without changing the original separator', async () => {
  const writes = native(); render(<Example />);
  const field = await screen.findByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement;
  fireEvent.change(field, { target: { value: '\nRead a page.\n' } });
  expect(field.value).toBe('\nRead a page.\n');
  await waitFor(() => expect(writes.at(-1)).toBe(agent.replace('Read a page.', '\nRead a page.')));
  fireEvent.change(field, { target: { value: 'Read a page.\n' } });
  expect(field.value).toBe('Read a page.\n');
  await waitFor(() => expect(writes.at(-1)).toBe(agent));
});
test('removing the separator in File view does not hide a later Agent-view newline', async () => {
  native(); const user = userEvent.setup(); render(<Example />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await user.click(screen.getByRole('button', { name: 'File' }));
  const file = screen.getByRole('textbox', { name: 'Agent file' });
  fireEvent.change(file, { target: { value: agent.replace('---\n\nRead', '---\nRead') } }); fireEvent.blur(file);
  await waitFor(() => expect(screen.queryByText('Saving…')).toBeNull());
  await user.click(screen.getByRole('button', { name: 'Agent' }));
  const field = screen.getByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement;
  fireEvent.change(field, { target: { value: '\nRead a page.\n' } });
  expect(field.value).toBe('\nRead a page.\n');
});
test('a no-op visit, disclosure and switches never write the file', async () => {
  const writes = native(agent.replaceAll('\n', '\r\n')); const user = userEvent.setup(); render(<Example />);
  await user.click(await screen.findByRole('textbox', { name: 'Instructions' }));
  await user.click(screen.getByText('More settings'));
  fireEvent.blur(screen.getByRole('textbox', { name: 'Description' }));
  await user.click(screen.getByRole('button', { name: 'File' }));
  await user.click(screen.getByRole('button', { name: 'Agent' }));
  expect(writes).toEqual([]);
});
test('keyboard picker adds and removes built-ins, keeping original bytes and focus', async () => {
  const writes = native(); const user = userEvent.setup(); render(<Example catalogKey={1} />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Add a tool' }).matches(':disabled')).toBe(false));
  expect(screen.getByText('Read a web page')).toBeTruthy();
  expect(screen.queryByRole('button', { name: /Read a file/ })).toBeNull();
  const add = screen.getByRole('button', { name: 'Add a tool' }); add.focus(); await user.keyboard('{Enter}');
  await user.type(screen.getByRole('searchbox', { name: 'Find a tool' }), 'file_read');
  await user.tab(); await user.keyboard('{Enter}');
  await waitFor(() => expect(writes.length).toBe(1));
  expect(writes[0]).toContain('future: {keep: yes}');
  expect(document.activeElement).toBe(add);
  screen.getByRole('button', { name: 'Remove Read a file' }).focus();
  await user.keyboard('{Enter}'); expect(document.activeElement).toBe(add);
  await waitFor(() => expect(writes.at(-1)).toBe(agent));
  await user.click(add); await user.keyboard('{Escape}'); expect(document.activeElement).toBe(add);
});
test('picker closes with a labeled X and returns focus without saving', async () => {
  const writes = native(); const user = userEvent.setup(); render(<Example catalogKey={1} />);
  const add = await screen.findByRole('button', { name: 'Add a tool' });
  await waitFor(() => expect(add.matches(':disabled')).toBe(false));
  await user.click(add);
  const close = screen.getByRole('button', { name: 'Close picker' });
  expect(close.querySelector('svg')?.getAttribute('aria-hidden')).toBe('true');
  expect(close.textContent).toBe('');
  await user.click(close);
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(document.activeElement).toBe(add);
  expect(writes).toEqual([]);
});
test('catalog unavailable preserves model while soul remains editable without catalog', async () => {
  native(); const user = userEvent.setup(); render(<Example />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  expect(screen.getByRole('combobox', { name: 'It thinks with' }).matches(':disabled')).toBe(true);
  expect(screen.getByRole('option', { name: 'file-model' })).toBeTruthy(); expect(screen.getByText(/Models unavailable/)).toBeTruthy();
  expect(screen.queryByText('through GitHub Copilot')).toBeNull();
  expect(screen.getByRole('button', { name: 'Add a tool' }).matches(':disabled')).toBe(true);
  await user.click(screen.getByText('More settings'));
  expect((screen.getByRole('textbox', { name: 'Personality' }) as HTMLTextAreaElement).readOnly).toBe(false);
  expect(screen.getByText(path)).toBeTruthy();
  fireEvent.change(screen.getByRole('textbox', { name: 'Description' }), { target: { value: 'A new description' } });
  await screen.findByRole('heading', { name: 'reader' });
  await waitFor(() => expect(screen.getByText('A new description', { selector: 'p' })).toBeTruthy());
});
test('every known built-in from the pinned Orka source renders a plain label before its name', async () => {
  // KnownBuiltInToolNames union at image pin 3cb1f0df36705af9ff81cb8b4e8a0f8d36ee6eea.
  const names = 'web_fetch web_search file_read file_write code_exec request_approval reply_in_conversation delegate_task wait_for_tasks create_container_task cancel_task send_message check_messages create_pull_request check_pull_request_ci merge_pull_request auto_merge_pull_request review_pull_request post_review_comment check_pr_review_marker list_issues list_pull_requests get_issue comment_on_issue create_agent delete_agent update_agent update_plan recall_memory remember propose_memory search_transcript create_ai_task create_pr_monitor create_agent_task check_task_progress fetch_task_output wait_for_task list_agents list_tools list_tasks list_connections create_tool delete_tool delete_session'.split(' ');
  native(agent.replace('    - kind: builtin\n      name: web_fetch\n', names.map(name => `    - kind: builtin\n      name: ${name}\n`).join('')));
  render(<Example />); await screen.findByRole('textbox', { name: 'Instructions' });
  const pills = document.querySelectorAll('.tool-pill'); expect(pills.length).toBe(names.length);
  names.forEach((name, index) => {
    expect(pills[index].querySelector('code')?.textContent).toBe(name);
    expect(pills[index].firstElementChild?.textContent).not.toBe(name);
  });
});
test('unknown built-in names cannot resolve inherited map properties', async () => {
  native(agent.replace('name: web_fetch', 'name: __proto__')); render(<Example />);
  expect(await screen.findByText('__proto__')).toBeTruthy();
  expect(screen.getByRole('textbox', { name: 'Instructions' })).toBeTruthy();
});
test('editing a commented description keeps it in the heading', async () => {
  native(agent.replace('description: "Reads a page"', 'description: Original # keep')); const user = userEvent.setup(); render(<Example />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await user.click(screen.getByText('More settings'));
  fireEvent.change(screen.getByRole('textbox', { name: 'Description' }), { target: { value: 'Edited' } });
  expect(screen.getByText('Edited', { selector: 'p' })).toBeTruthy();
});
test('setup opens the owned document for editing but keeps discovery and Run gated', async () => {
  const writes = native(); const user = userEvent.setup(); render(<Example setupPending catalogKey={1} />);
  const instructions = await screen.findByRole('textbox', { name: 'Instructions' });
  expect((instructions as HTMLTextAreaElement).readOnly).toBe(false);
  expect(screen.getByRole('button', { name: 'Add a tool' }).matches(':disabled')).toBe(true);
  expect(screen.getByRole('combobox').matches(':disabled')).toBe(true);
  expect(screen.getByRole('button', { name: 'Run tests' }).matches(':disabled')).toBe(true);
  await user.click(screen.getByText('More settings'));
  expect(screen.getByText(path)).toBeTruthy();
  fireEvent.change(instructions, { target: { value: 'Edited during setup.' } }); fireEvent.blur(instructions);
  await waitFor(() => expect(writes).toEqual([agent.replace('Read a page.\n', 'Edited during setup.')]));
});
test.each([42, '', 'bad\0path', 'x'.repeat(32769)])('rejects malformed outbound display paths (%j)', async path => {
  vi.stubGlobal('isTauri', true); mockIPC(() => ({ agent, tests: 'Checks', path }));
  await expect(agentCall('agent_open')).rejects.toThrow('Your agent folder could not be opened.');
});
test('form edits preserve mixed line endings outside the edited body', async () => {
  const mixed = agent.replace('description:', '\r\ndescription:'); const writes = native(mixed); const user = userEvent.setup(); render(<Example />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await user.click(screen.getByText('More settings'));
  fireEvent.change(screen.getByRole('textbox', { name: 'Description' }), { target: { value: 'Changed' } });
  await waitFor(() => expect(writes).toEqual([mixed.replace('"Reads a page"', '"Changed"')]));
});
