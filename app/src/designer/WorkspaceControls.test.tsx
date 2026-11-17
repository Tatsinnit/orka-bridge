import { openLocalSetup } from '../setup/test-navigation';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import AgentParts from './AgentParts';
import Example from './Example';
import App from '../App';
import { readForm } from './agent-form';
import type { WorkspaceCatalogReply } from '../catalog';
const source = '---\nname: reader\nmodel: unlisted\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools: []\n---\nRole only.\n';
const catalog: WorkspaceCatalogReply = { defaultModel: 'workspace-default', models: { state: 'ready', items: [{ id: 'listed' }, { id: 'hidden', pickerEnabled: false }, { id: 'no-chat', chatSupported: false }] }, tools: { state: 'ready', items: [
  { kind: 'builtin', name: 'web_fetch', description: 'Read an address.' },
  { kind: 'tool', name: 'lookup', description: '<b>Workspace lookup</b>', available: false },
  { kind: 'tool', name: 'web_fetch', description: 'Must not offer this target' },
] } };
beforeEach(() => { Object.defineProperty(HTMLDivElement.prototype, 'showPopover', { configurable: true, value() { this.style.display = 'grid'; } }); });
afterEach(() => { cleanup(); clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); localStorage.clear(); Reflect.deleteProperty(HTMLDivElement.prototype, 'showPopover'); });
test('catalog model choices retain unlisted file model and never substitute the default', () => {
  const changed = vi.fn(); render(<AgentParts source={source} readOnly={false} catalog={catalog} onChange={changed} />);
  const model = screen.getByRole('combobox', { name: 'It thinks with' });
  expect((model as HTMLSelectElement).value).toBe('unlisted');
  expect(screen.getByRole('option', { name: /unlisted.*unlisted/i })).toBeTruthy();
  expect(screen.getByText(/It may not run here/)).toBeTruthy();
  expect(screen.queryByRole('option', { name: 'hidden' })).toBeNull();
  expect(screen.queryByRole('option', { name: 'no-chat' })).toBeNull();
  expect(changed).not.toHaveBeenCalled();
  fireEvent.change(model, { target: { value: 'listed' } });
  expect(readForm(changed.mock.calls[0][0]).model).toBe('listed');
});
test('workspace default follows the connection without replacing a saved model until chosen', () => {
  const changed = vi.fn();
  const withDefault = { ...catalog, defaultModel: 'listed' };
  const view = render(<AgentParts source={source} readOnly={false} catalog={withDefault} onChange={changed} />);
  expect(screen.getByRole('option', { name: 'Workspace default: listed' }).getAttribute('value')).toBe('listed');
  const next = { ...catalog, defaultModel: 'another', models: { state: 'ready' as const, items: [{ id: 'listed' }, { id: 'another' }] } };
  view.rerender(<AgentParts source={source} readOnly={false} catalog={next} onChange={changed} />);
  expect((screen.getByRole('combobox') as HTMLSelectElement).value).toBe('unlisted');
  expect(screen.getByRole('option', { name: 'Workspace default: another' })).toBeTruthy();
  expect(changed).not.toHaveBeenCalled();
  fireEvent.change(screen.getByRole('combobox'), { target: { value: 'another' } });
  expect(changed).toHaveBeenCalledExactlyOnceWith(source.replace('model: unlisted', 'model: "another"'));
});
test('bounded first-page notice counts returned rows, including excluded models', () => {
  const rows = Array.from({ length: 1000 }, (_, i) => ({ id: `m-${i}`, pickerEnabled: false }));
  const changed = vi.fn();
  const view = render(<AgentParts source={source} readOnly={false} catalog={{ ...catalog, models: { state: 'ready', items: rows } }} onChange={changed} />);
  expect(screen.getByText('Showing up to 1,000 models; more may be available.')).toBeTruthy();
  expect(screen.getAllByRole('option')).toHaveLength(1);
  view.rerender(<AgentParts source={source} readOnly={false} catalog={{ ...catalog, models: { state: 'ready', items: rows.slice(0, 999) } }} onChange={changed} />);
  expect(screen.queryByText(/more may be available/)).toBeNull();
  expect(changed).not.toHaveBeenCalled();
});
test('remote configured defaults keep their model label and do not claim first-page discovery', () => {
  const changed = vi.fn();
  const rows = Array.from({ length: 1000 }, (_, i) => ({ id: i === 0 ? 'listed' : `m-${i}` }));
  render(<AgentParts source={source} readOnly={false} remote catalog={{ ...catalog, defaultModel: 'listed', models: { state: 'ready', items: rows } }} onChange={changed} />);
  expect(screen.getByRole('option', { name: 'listed' })).toBeTruthy();
  expect(screen.queryByRole('option', { name: /Workspace default/ })).toBeNull();
  expect(screen.queryByText(/more may be available/)).toBeNull();
  expect(changed).not.toHaveBeenCalled();
});
test('an existing incompatible model stays visible without calling it unlisted', () => {
  const changed = vi.fn(); render(<AgentParts source={source.replace('model: unlisted', 'model: hidden')} readOnly={false} catalog={catalog} onChange={changed} />);
  expect(screen.getByRole('option', { name: 'hidden (not offered for chat)' })).toBeTruthy();
  expect(screen.getByText('This model is not offered for chat in Bridge. Choose another model to use it here.')).toBeTruthy();
  expect(changed).not.toHaveBeenCalled();
});
test('tool choices show descriptions as text, distinguish kinds and suppress effective-name collisions', async () => {
  const changed = vi.fn(); const user = userEvent.setup(); const view = render(<AgentParts source={source} readOnly={false} catalog={catalog} onChange={changed} />);
  const add = screen.getByRole('button', { name: 'Add a tool' }); await user.click(add);
  expect(screen.getByText('Read the contents of a web page at an address.')).toBeTruthy();
  expect(screen.getByText('<b>Workspace lookup</b>')).toBeTruthy();
  expect(document.querySelector('.tool-picker b')).toBeNull();
  expect(screen.queryByText('Must not offer this target')).toBeNull();
  expect(screen.getByText(/Observed unavailable/)).toBeTruthy();
  await user.click(screen.getByRole('button', { name: /lookup.*Cluster tool/i }));
  const edited = changed.mock.calls[0][0]; expect(readForm(edited).tools[0]).toMatchObject({ name: 'lookup', kind: 'tool' });
  expect(document.activeElement).toBe(add);
  view.rerender(<AgentParts source={edited} readOnly={false} catalog={catalog} onChange={changed} />);
  await user.click(add); expect(screen.queryByRole('button', { name: /lookup.*Cluster tool/i })).toBeNull();
  await user.keyboard('{Escape}'); expect(document.activeElement).toBe(add);
  await user.click(screen.getByRole('button', { name: 'Remove lookup' }));
  expect(readForm(changed.mock.calls.at(-1)![0]).tools).toEqual([]);
});
test.each(['__proto__', 'constructor', 'toString'])('unknown built-in %s renders its supplied description, not inherited map values', async name => {
  const user = userEvent.setup(); const changed = vi.fn();
  render(<AgentParts source={source} readOnly={false} catalog={{ ...catalog, tools: { state: 'ready', items: [{ kind: 'builtin', name, description: 'Supplied tool description' }] } }} onChange={changed} />);
  await user.click(screen.getByRole('button', { name: 'Add a tool' }));
  expect(screen.getByText('Supplied tool description')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: new RegExp(name) }));
  expect(readForm(changed.mock.calls[0][0]).tools[0]).toMatchObject({ name, kind: 'builtin' });
});
test.each(['builtin', 'tool'] as const)('approval %s is explained but cannot be added, even through search', async kind => {
  const user = userEvent.setup(); const changed = vi.fn();
  render(<AgentParts source={source} readOnly={false} catalog={{ ...catalog, tools: { state: 'ready', items: [{ kind, name: 'request_approval', description: 'Ask a person.' }] } }} onChange={changed} />);
  await user.click(screen.getByRole('button', { name: 'Add a tool' }));
  await user.type(screen.getByRole('searchbox'), 'request_approval');
  const option = screen.getByRole('button', { name: /request_approval/ });
  expect(option.matches(':disabled')).toBe(true);
  expect(screen.getByText('Approval tools need autonomous coordination, which Bridge does not support for local tests or Chat.')).toBeTruthy();
  await user.click(option);
  expect(changed).not.toHaveBeenCalled();
});
test('a saved approval declaration stays visible and unchanged until explicitly removed', async () => {
  const user = userEvent.setup(); const changed = vi.fn();
  const saved = source.replace('tools: []', 'tools: [{kind: builtin, name: request_approval}]');
  render(<AgentParts source={saved} readOnly={false} onChange={changed} />);
  expect(screen.getByText('Ask for approval')).toBeTruthy();
  expect(screen.getByText(/Remove all request_approval and reply_in_conversation tools before running local tests or Chat/)).toBeTruthy();
  expect(changed).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Remove Ask for approval' }));
  expect(readForm(changed.mock.calls[0][0]).tools).toEqual([]);
});
test('tool search includes the friendly description shown in the picker', async () => {
  const user = userEvent.setup(); render(<AgentParts source={source} readOnly={false} catalog={catalog} onChange={() => {}} />);
  await user.click(screen.getByRole('button', { name: 'Add a tool' }));
  await user.type(screen.getByRole('searchbox'), 'contents');
  expect(screen.getByRole('button', { name: /Read a web page/ })).toBeTruthy();
});
test('a restored catalog never reopens a picker dismissed by unavailability', async () => {
  const user = userEvent.setup();
  const view = render(<AgentParts source={source} readOnly={false} catalog={catalog} onChange={() => {}} />);
  await user.click(screen.getByRole('button', { name: 'Add a tool' }));
  await user.type(screen.getByRole('searchbox'), 'lookup');
  view.rerender(<AgentParts source={source} readOnly={false} catalogLoading onChange={() => {}} />);
  expect(screen.queryByRole('searchbox')).toBeNull();
  view.rerender(<AgentParts source={source} readOnly={false} catalog={catalog} onChange={() => {}} />);
  expect(screen.queryByRole('searchbox')).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Add a tool' }));
  expect((screen.getByRole('searchbox') as HTMLInputElement).value).toBe('');
});
test('independent catalog failures and empty lists do not fall back to static inventory', async () => {
  const user = userEvent.setup(); const view = render(<AgentParts source={source} readOnly={false} onChange={() => {}} />);
  expect(screen.getByRole('combobox', { name: 'It thinks with' }).matches(':disabled')).toBe(true);
  expect(screen.getByRole('button', { name: 'Add a tool' }).matches(':disabled')).toBe(true);
  expect(screen.getAllByText(/Couldn't read (tools|models) from This computer/).length).toBeGreaterThan(0);
  view.rerender(<AgentParts source={source} readOnly={false} catalog={{ ...catalog, models: { state: 'unavailable' }, tools: { state: 'ready', items: [] } }} onChange={() => {}} />);
  await user.click(screen.getByRole('button', { name: 'Add a tool' }));
  expect(screen.getByText('No tools listed by this cluster.')).toBeTruthy();
  expect(screen.queryByRole('button', { name: /Read a file/ })).toBeNull();
  view.rerender(<AgentParts source={source} readOnly={false} catalog={{ ...catalog, models: { state: 'ready', items: [] }, tools: { state: 'unavailable' } }} onChange={() => {}} />);
  expect(screen.getByText('No models listed by the workspace.')).toBeTruthy();
});
test('inline soul and digest publish together, and reference fields never switch source', async () => {
  const changed = vi.fn(); const user = userEvent.setup();
  const inline = source.replace('  tools:', '  soul: {inline: old, digest: "sha256:' + '0'.repeat(64) + '"}\n  tools:');
  const view = render(<AgentParts source={inline} readOnly={false} onChange={changed} />);
  await user.click(screen.getByText('More settings'));
  fireEvent.change(screen.getByRole('textbox', { name: 'Personality' }), { target: { value: 'Kind.' } });
  await waitFor(() => expect(changed).toHaveBeenCalledTimes(1));
  expect(readForm(changed.mock.calls[0][0]).soul).toEqual({ inline: 'Kind.', digest: 'sha256:f9fcd6e31bd6311467d29894fae0db1c16acb84bfe9cfb30b666ea77eec43b69' });
  const ref = source.replace('  tools:', '  soul: {configMapRef: {name: persona, key: SOUL.md}, digest: sha256:old}\n  tools:');
  view.rerender(<AgentParts source={ref} readOnly={false} onChange={changed} />);
  expect(screen.queryByRole('textbox', { name: 'Personality' })).toBeNull();
  fireEvent.change(screen.getByRole('textbox', { name: 'ConfigMap name' }), { target: { value: 'persona.v2' } });
  expect(readForm(changed.mock.calls.at(-1)![0]).soul?.configMapRef?.name).toBe('persona.v2');
  expect(screen.getByRole('textbox', { name: 'Expected digest' })).toBeTruthy();
});
test('a pending digest cannot overwrite a newer File edit', async () => {
  let finish!: (value: ArrayBuffer) => void;
  vi.spyOn(crypto.subtle, 'digest').mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const changed = vi.fn(); const user = userEvent.setup();
  const inline = source.replace('  tools:', '  soul: {inline: old, digest: sha256:old}\n  tools:');
  const view = render(<AgentParts source={inline} readOnly={false} onChange={changed} />);
  await user.click(screen.getByText('More settings'));
  fireEvent.change(screen.getByRole('textbox', { name: 'Personality' }), { target: { value: 'Pending' } });
  view.rerender(<AgentParts source={source} readOnly={false} onChange={changed} />);
  expect(screen.getByRole('alert').textContent).toMatch(/newer edit/i);
  view.rerender(<AgentParts source={inline} readOnly={false} onChange={changed} />);
  expect((screen.getByRole('textbox', { name: 'Personality' }) as HTMLTextAreaElement).value).toBe('old');
  await act(async () => finish(new ArrayBuffer(32)));
  expect(changed).not.toHaveBeenCalled();
});
test('pending soul edits survive File view and block running until the atomic update is saved', async () => {
  let finish!: (value: ArrayBuffer) => void; const saves: string[] = [];
  vi.spyOn(crypto.subtle, 'digest').mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  vi.stubGlobal('isTauri', true);
  const inline = source.replace('  tools:', '  soul: {inline: old, digest: sha256:old}\n  tools:');
  mockIPC((command, args) => {
    if (command === 'agent_open') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: inline, tests: 'Checks' };
    if (command === 'agent_save') { saves.push((args as { content: string }).content); return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: saves.at(-1), tests: 'Checks' }; }
    return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'idle', canRun: true, report: null, acknowledged: [], pending: null, error: null };
  });
  const user = userEvent.setup(); render(<Example />);
  const run = await screen.findByRole('button', { name: 'Run tests' });
  await waitFor(() => expect(run.matches(':disabled')).toBe(false));
  await user.click(screen.getByText('More settings'));
  fireEvent.change(screen.getByRole('textbox', { name: 'Personality' }), { target: { value: 'New text' } });
  expect(run.matches(':disabled')).toBe(true);
  await user.click(screen.getByRole('button', { name: 'File' }));
  await act(async () => finish(new ArrayBuffer(32)));
  await waitFor(() => expect(saves.length).toBe(1));
  expect(readForm(saves[0]).soul?.inline).toBe('New text');
  expect((screen.getByRole('textbox', { name: 'Agent file' }) as HTMLTextAreaElement).value).toBe(saves[0]);
  await waitFor(() => expect(run.matches(':disabled')).toBe(false));
});
test('removing the last tool without a catalog returns focus to the Add placeholder', async () => {
  const selected = source.replace('tools: []', 'tools: [{kind: tool, name: lookup}]');
  const changed = vi.fn(); const user = userEvent.setup();
  const view = render(<AgentParts source={selected} readOnly={false} onChange={changed} />);
  await user.click(screen.getByRole('button', { name: 'Remove lookup' }));
  view.rerender(<AgentParts source={changed.mock.calls[0][0]} readOnly={false} onChange={changed} />);
  expect(document.activeElement?.getAttribute('aria-label')).toMatch(/^Add a tool:/);
});
test('a failed catalog can be refreshed without changing the file', async () => {
  vi.stubGlobal('isTauri', true); let calls = 0;
  mockIPC(command => {
    if (command === 'agent_open') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: source, tests: 'Checks' };
    if (command === 'workspace_catalog') { if (++calls === 1) throw Error('temporary'); return catalog; }
    return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
  });
  const user = userEvent.setup(); render(<Example catalogKey={1} />);
  await user.click(await screen.findByRole('button', { name: 'Refresh tools and models' }));
  await screen.findByRole('option', { name: 'listed' });
  expect((screen.getByRole('combobox', { name: 'It thinks with' }) as HTMLSelectElement).value).toBe('unlisted');
});
test('explicit Stop invalidates catalog before the stop reply, retaining the editor', async () => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  let finish!: (value: unknown) => void;
  const ready = { selected: 'localVm', localVm: 'ready', model: 'available', localVmError: null };
  let current = ready;
  mockIPC(command => {
    if (command === 'setup_status') return current;
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'setup_cancel_local') return new Promise(resolve => { finish = resolve; });
    if (command === 'workspace_catalog') return catalog;
    if (command === 'setup_example') return { agent: source, tests: 'Checks' };
    if (command === 'agent_open') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: source, tests: 'Checks' };
    return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await within(screen.getByRole('region', { name: 'Connect the AI' })).findByText('Connected');
  await user.click(screen.getByRole('button', { name: 'Read its instructions' }));
  await screen.findByRole('option', { name: 'listed' });
  await user.click(screen.getByRole('button', { name: /Clusters/ }));
  await user.click(screen.getByRole('button', { name: 'Stop the workspace' }));
  await user.click(screen.getByRole('button', { name: 'Stop workspace' }));
  // Designer stays mounted under the inactive page. It must not retain old choices.
  expect(document.querySelector('option[value="listed"]')).toBeNull();
  expect(document.querySelector('.instructions-editor')?.textContent).toBe('Role only.\n');
  await act(async () => { current = { ...ready, localVm: 'cancelled', model: 'locked' }; finish(current); });
  expect(await screen.findByRole('button', { name: 'Start' })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect((screen.getByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement).disabled).toBe(false);
});
test('workspace generation invalidates a late catalog without losing the owned draft', async () => {
  const pending: ((value: unknown) => void)[] = []; vi.stubGlobal('isTauri', true);
  mockIPC(command => {
    if (command === 'agent_open') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: source, tests: 'Checks' };
    if (command === 'workspace_catalog') return new Promise(resolve => pending.push(resolve));
    return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
  });
  const view = render(<Example catalogKey={1} />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await waitFor(() => expect(pending.length).toBe(1));
  view.rerender(<Example catalogKey={2} />);
  await waitFor(() => expect(pending.length).toBe(2));
  await act(async () => pending[0](catalog));
  expect(screen.queryByRole('option', { name: 'listed' })).toBeNull();
  await act(async () => pending[1](catalog));
  expect(screen.getByRole('option', { name: 'listed' })).toBeTruthy();
  view.rerender(<Example />);
  expect(screen.queryByRole('option', { name: 'listed' })).toBeNull();
  expect((screen.getByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement).value).toBe('Role only.\n');
});
const named: WorkspaceCatalogReply = { ...catalog, defaultModel: 'gpt-5.6-terra', models: { state: 'ready', items: [{ id: 'gpt-5.6-terra' }, { id: 'custom-model-7' }] } };
test('known model IDs get display names while option values, saved source and emitted IDs stay exact', () => {
  const changed = vi.fn(); render(<AgentParts source={source.replace('model: unlisted', 'model: gpt-5.6-terra')} readOnly={false} catalog={named} onChange={changed} />);
  const model = screen.getByRole('combobox', { name: 'It thinks with' }) as HTMLSelectElement;
  expect(model.value).toBe('gpt-5.6-terra');
  const known = screen.getByRole('option', { name: 'Workspace default: GPT-5.6 Terra' }) as HTMLOptionElement; expect(known.value).toBe('gpt-5.6-terra');
  expect(screen.queryByRole('option', { name: 'gpt-5.6-terra' })).toBeNull();
  expect((screen.getByRole('option', { name: 'custom-model-7' }) as HTMLOptionElement).value).toBe('custom-model-7');
  expect(document.body.textContent).not.toMatch(/GitHub|Copilot|OpenAI/i);
  expect(changed).not.toHaveBeenCalled();
  fireEvent.change(model, { target: { value: 'custom-model-7' } }); expect(readForm(changed.mock.calls[0][0]).model).toBe('custom-model-7');
  fireEvent.change(model, { target: { value: 'gpt-5.6-terra' } }); expect(changed).toHaveBeenCalledTimes(1);
});
test('selecting the friendly option writes the exact ID', () => {
  const changed = vi.fn(); render(<AgentParts source={source} readOnly={false} catalog={named} onChange={changed} />);
  fireEvent.change(screen.getByRole('combobox', { name: 'It thinks with' }), { target: { value: 'gpt-5.6-terra' } });
  expect(readForm(changed.mock.calls[0][0]).model).toBe('gpt-5.6-terra');
});
test('unlisted and unavailable known models keep their friendly name and explanations', () => {
  const view = render(<AgentParts source={source.replace('model: unlisted', 'model: gpt-5.6-terra')} readOnly={false} catalog={{ ...named, models: { state: 'ready', items: [{ id: 'listed' }] } }} onChange={() => {}} />);
  expect(screen.getByRole('option', { name: 'GPT-5.6 Terra (unlisted)' })).toBeTruthy(); expect(screen.getByText(/It may not run here/)).toBeTruthy();
  view.rerender(<AgentParts source={source.replace('model: unlisted', 'model: gpt-5.6-terra')} readOnly={false} catalog={{ ...named, models: { state: 'ready', items: [{ id: 'gpt-5.6-terra', chatSupported: false }, { id: 'listed' }] } }} onChange={() => {}} />);
  expect(screen.getByRole('option', { name: 'GPT-5.6 Terra (not offered for chat)' })).toBeTruthy(); expect(screen.getByText(/not offered for chat in Bridge/)).toBeTruthy();
});
test('a listed local model default is not repeated in helper text', () => {
  render(<AgentParts source={source.replace('model: unlisted', 'model: gpt-5.6-terra')} readOnly={false} catalog={named} onChange={() => {}} />);
  expect(screen.queryByText(/Default model/)).toBeNull(); expect(screen.getAllByText(/GPT-5\.6 Terra|gpt-5\.6-terra/)).toHaveLength(1);
});
