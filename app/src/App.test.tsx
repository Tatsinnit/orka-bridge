import { noUpgrade } from './setup/test-upgrade';
import { openLocalSetup } from './setup/test-navigation';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from './App';

beforeEach(() => { localStorage.setItem('bridge.welcomeSeen.v1', '1'); Object.defineProperty(HTMLDivElement.prototype, 'showPopover', { configurable: true, value() { this.style.display = 'grid'; } }); });
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); Reflect.deleteProperty(HTMLDivElement.prototype, 'showPopover'); });

test.each(['Agent', 'File'] as const)('a pending %s draft survives setup readiness without reopening the document', async view => {
  const pair = { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: '---\nname: reader\n---\nOriginal instructions.', tests: 'Checks', path: '/owned/reader.agent.md' };
  let localVm = 'starting'; let opens = 0; const discovery: string[] = [];
  let finish!: (value: unknown) => void; let pending = '';
  vi.stubGlobal('isTauri', true);
  mockIPC((command, args) => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm, model: localVm === 'ready' ? 'available' : 'locked' };
    if (command === 'setup_example') return { agent: pair.agent, tests: pair.tests };
    if (command === 'agent_open') { opens++; return pair; }
    if (command === 'agent_save') { pending = (args as { content: string }).content; return new Promise(resolve => { finish = resolve; }); }
    if (command === 'workspace_catalog' || command === 'agent_run') { discovery.push(command); throw Error('Must stay gated'); }
    if (command === 'agent_run_status') return { documentId: pair.documentId, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  const user = userEvent.setup(); render(<App />);
  await within(screen.getByRole('contentinfo')).findByText('Getting your workspace ready, step 2 of 4');
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  await screen.findByRole('textbox', { name: 'Instructions' });
  if (view === 'File') await user.click(screen.getByRole('button', { name: 'File' }));
  const editor = screen.getByRole('textbox', { name: view === 'File' ? 'Agent file' : 'Instructions' }) as HTMLTextAreaElement;
  const edited = view === 'File' ? pair.agent.replace('Original instructions.', 'Draft kept through setup.') : 'Draft kept through setup.';
  fireEvent.change(editor, { target: { value: edited } }); fireEvent.blur(editor);
  await waitFor(() => expect(pending).toContain('Draft kept through setup.'));
  localVm = 'ready';
  await within(screen.getByRole('contentinfo')).findByText('Connect the AI, step 3 of 4', {}, { timeout: 2000 });
  expect(screen.getByRole('textbox', { name: view === 'File' ? 'Agent file' : 'Instructions' })).toBe(editor);
  expect(editor.value).toBe(edited); expect(opens).toBe(1); expect(discovery).toEqual([]);
  expect(screen.getByRole('button', { name: 'Run tests' }).matches(':disabled')).toBe(true);
  await act(async () => finish({ ...pair, agent: pending }));
  await within(screen.getByRole('contentinfo')).findByText('Saved');
  expect(editor.value).toBe(edited);
});

test.each([true, false])('reopening Designer uses native model connection evidence (%s)', async modelConnected => {
  vi.stubGlobal('isTauri', true); const catalogs = vi.fn();
  const id = '9b2b93c4-375c-4815-9717-39852a96e411';
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'agent_open') return { documentId: id, name: 'reader', agent: '---\nname: reader\nmodel: file-model\nkmx:\n  runtime: orka\n  schemaVersion: 1\n  tools: []\n---\nRead.', tests: 'Checks' };
    if (command === 'agent_run_status') return { documentId: id, busy: null, state: 'idle', canRun: modelConnected, modelConnected, report: null, acknowledged: [], pending: null, error: null };
    if (command === 'workspace_catalog') { catalogs(); return { defaultModel: 'file-model', models: { state: 'ready', items: [{ id: 'file-model' }] }, tools: { state: 'ready', items: [] } }; }
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  await screen.findByRole('textbox', { name: 'Instructions' });
  if (modelConnected) await waitFor(() => expect(screen.getByRole('button', { name: 'Add a tool' }).matches(':disabled')).toBe(false));
  else { await screen.findByText(/Connect the AI in Clusters first/); expect(screen.getByRole('button', { name: 'Add a tool' }).matches(':disabled')).toBe(true); }
  expect(catalogs.mock.calls.length).toBe(modelConnected ? 1 : 0);
});

test.each(['preparing', 'installing', 'starting', 'closing', 'failed'])('Overview reflects setup %s rather than a stopped cluster', async localVm => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm, model: 'locked' };
    if (command === 'overview_files') return { state: 'complete', files: [], runActive: false, historyState: 'complete', runAgents: [] };
    if (command === 'overview_places') return { places: [], historyState: 'complete', unobserved: [] };
    if (command === 'overview_agents') return { state: 'unavailable', installation: null, agents: [], observedAt: null, observation: null, runActive: false, reason: 'stopped' };
    return null;
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  const page = await screen.findByRole('main', { name: 'Your agents' });
  const expected = localVm === 'starting' ? 'Starting the workspace' : localVm === 'closing' ? 'Stopping the workspace' : localVm === 'failed' ? 'Setup failed' : 'Getting the workspace ready';
  const overview = within(page).getByRole('region', { name: 'Your agents' });
  expect(await within(overview).findByText(expected)).toBeTruthy();
  expect(within(overview).queryByText(/The workspace is stopped/)).toBeNull();
});

test.each(['closed', 'failed'])('Overview shows a pending start before the first reply from %s', async localVm => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm, model: 'locked' };
    if (command === 'setup_select_local') return new Promise(() => {});
    if (command === 'overview_files') return { state: 'complete', files: [], runActive: false, historyState: 'complete', runAgents: [] };
    if (command === 'overview_places') return { places: [], historyState: 'complete', unobserved: [] };
    if (command === 'overview_agents') return { state: 'unavailable', installation: null, agents: [], observedAt: null, observation: null, runActive: false, reason: 'stopped' };
    return null;
  });
  const user = userEvent.setup(); render(<App />);
  await openLocalSetup(user);
  await user.click(await screen.findByRole('button', { name: localVm === 'failed' ? 'Try again' : 'Start again' }));
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  const overview = screen.getByRole('region', { name: 'Your agents' });
  expect(await within(overview).findByText('Getting your workspace ready…')).toBeTruthy();
  expect(within(overview).queryByText(/stopped|Setup failed/i)).toBeNull();
});

test('navigation_order', () => {
  render(<App />);
  const nav = screen.getByRole('navigation', { name: 'Main' });
  expect(within(nav).getAllByRole('button').map(button => button.getAttribute('aria-label')))
    .toEqual(['Overview', 'Designer', 'Clusters']);
  expect(screen.getByRole('img', { name: 'Orka' })).toBeTruthy();
  expect(screen.getByText('Bridge')).toBeTruthy();
});

test('selected_heading', async () => {
  const user = userEvent.setup();
  render(<App />);
  expect(await screen.findByRole('heading', { name: 'Clusters' })).toBeTruthy();
  for (const name of ['Designer', 'Clusters', 'Overview']) {
    await user.click(screen.getByRole('button', { name }));
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
    expect(await screen.findByRole('main', { name: name === 'Clusters' ? 'Clusters' : name === 'Overview' ? 'Your agents' : name })).toBeTruthy();
    const selected = screen.getAllByRole('button').filter(button => button.getAttribute('aria-current') === 'page');
    expect(selected).toEqual([screen.getByRole('button', { name })]);
  }
});

test('keyboard_activation', async () => {
  const user = userEvent.setup();
  render(<App />);
  await user.tab();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Overview' }));
  await user.tab();
  await user.keyboard(' ');
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('Designer');
  await user.tab();
  await user.keyboard('{Enter}');
  expect(await screen.findByRole('heading', { name: 'Clusters' })).toBeTruthy();
});

test('navigation starts at the page heading after scrolling another page', async () => {
  const user = userEvent.setup(); render(<App />);
  const main = screen.getByRole('main');
  main.scrollTop = 300;
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect(main.scrollTop).toBe(0);
});

test('Overview reads saved documents without opening, setup or VM commands; Open uses document identity', async () => {
  vi.stubGlobal('isTauri', true);
  const id = '12345678-1234-1234-1234-123456789abc'; const calls: string[] = [];
  mockIPC((command, args) => {
    calls.push(command);
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: null, localVm: 'idle', model: 'locked' };
    if (command === 'overview_files') return { state: 'complete', runActive: false, files: [{ documentId: id, name: 'second', state: 'readable', source: null, summary: null, reason: 'Declarations unavailable' }] };
    if (command === 'overview_agents') return { state: 'unavailable', installation: null, agents: [], observation: null, observedAt: null, runActive: false, reason: 'stopped' };
    if (command === 'agent_open') { expect(args).toEqual({ documentId: id }); return { documentId: id, name: 'second', agent: '---\nname: second\n---\nRead.', tests: 'Checks' }; }
    if (command === 'agent_run_status') return { documentId: id, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await screen.findByRole('heading', { name: 'second' });
  expect(calls).not.toContain('agent_open');
  expect(calls.some(c => /setup_select|agent_run$|send_start/.test(c))).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Open second' }));
  expect(await screen.findByRole('heading', { name: 'second', level: 1 })).toBeTruthy();
  // Native selection is still null, so the chip does not invent a place after Open.
  expect(document.querySelector('.host-chip')?.textContent).toContain('This computer');
  expect(document.querySelector('.host-chip .live-dot')?.className).not.toContain('ready');
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open second' }));
  expect(calls.filter(c => c === 'agent_open')).toHaveLength(1);
});

test('Overview fitting Chat routes to the saved document on This computer', async () => {
  const action = 'Chat';
  vi.stubGlobal('isTauri', true);
  const id = '12345678-1234-4234-8234-123456789abc';
  const installation = '22345678-1234-4234-8234-123456789abc';
  const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
  const calls: { command: string; args: unknown }[] = [];
  mockIPC((command, args) => {
    calls.push({ command, args });
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'overview_files') return { state: 'complete', runActive: false, historyState: 'complete', runAgents: [], files: [{ documentId: id, name: 'reader', source, state: 'readable', summary: { name: 'reader', description: 'Reads a page', model: 'model', tools: [] }, reason: null, localResult: { installation, documentId: id, source, namespace: 'orka-system', agentName: 'generated-reader', agentUid: 'run-id', observedAt: 123, state: 'complete', report: { verdict: 'passed', passed: 1, total: 1 } } }] };
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'overview_agents') return { state: 'complete', installation, agents: [], observation: null, observedAt: 123, runActive: false, reason: null };
    if (command === 'overview_places') return { places: [], historyState: 'complete', unobserved: [] };
    if (command === 'agent_open') { expect(args).toEqual({ documentId: id }); return { documentId: id, name: 'reader', source, agent: '---\nname: reader\n---\nRead.', tests: 'Checks' }; }
    if (command === 'agent_run_status' || command === 'agent_run') return { documentId: (args as { documentId: string }).documentId, busy: null, state: command === 'agent_run' ? 'running' : 'idle', canRun: command !== 'agent_run', modelConnected: true, source, report: null, acknowledged: [], pending: null, error: null };
    return null;
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await openLocalSetup(user);
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  const row = await screen.findByRole('article', { name: 'reader' });
  await user.click(within(row).getByRole('button', { name: 'Show places for reader' }));
  await user.click(within(row).getByRole('button', { name: `${action} reader` }));
  await screen.findByRole('heading', { name: 'reader', level: 1 });
  if (action === 'Chat') await waitFor(() => expect(screen.getByRole('button', { name: 'Chat', pressed: true })).toBeTruthy());
  else await waitFor(() => expect(calls.filter(call => call.command === 'agent_run')).toEqual([{ command: 'agent_run', args: { documentId: id } }]));
  expect(calls.some(call => call.command === 'send_start')).toBe(false);
});

test('saved sample Open edits the actual file during setup without admitting Run', async () => {
  vi.stubGlobal('isTauri', true); const calls: string[] = [];
  const id = '9b2b93c4-375c-4815-9717-39852a96e411';
  mockIPC(command => {
    calls.push(command);
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'starting', model: 'locked' };
    if (command === 'overview_files') return { state: 'complete', runActive: false, files: [{ documentId: id, name: 'web-reader', state: 'readable', source: null, summary: null, reason: null }] };
    if (command === 'setup_example') return { agent: '---\nname: bundled-example\n---\nBundled content', tests: 'Checks' };
    if (command === 'agent_open') return { documentId: id, name: 'web-reader', agent: '---\nname: saved-reader\n---\nMy saved changes', tests: 'Checks' };
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open web-reader' }));
  expect(await screen.findByRole('heading', { name: 'saved-reader', level: 1 })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'File' }));
  expect((screen.getByRole('textbox', { name: 'Agent file' }) as HTMLTextAreaElement).readOnly).toBe(false);
  expect(calls.some(c => /setup_select|agent_run$|send_start/.test(c))).toBe(false);
});

test('switching the owned example to a registered document opens each identity once', async () => {
  vi.stubGlobal('isTauri', true); const opened: string[] = [];
  const id = '12345678-1234-1234-1234-123456789abc';
  mockIPC((command, args) => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'starting', model: 'locked' };
    if (command === 'overview_files') return { state: 'complete', runActive: false, files: [{ documentId: id, name: 'other', state: 'readable', source: null, summary: null, reason: null }] };
    if (command === 'setup_example') return { agent: '---\nname: bundled-example\n---\nBundled content', tests: 'Checks' };
    if (command === 'agent_open') { const doc = (args as { documentId: string }).documentId; opened.push(doc); const name = doc === id ? 'other' : 'owned-sample'; return { documentId: doc, name, agent: `---\nname: ${name}\n---\nSaved content`, tests: 'Checks' }; }
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  await screen.findByRole('heading', { name: 'owned-sample', level: 1 });
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open other' }));
  await screen.findByRole('heading', { name: 'other', level: 1 });
  expect(opened).toEqual(['9b2b93c4-375c-4815-9717-39852a96e411', id]);
});

test('first opening the sample refreshes the hidden saved count without live discovery', async () => {
  vi.stubGlobal('isTauri', true);
  const id = '9b2b93c4-375c-4815-9717-39852a96e411';
  const pair = { documentId: id, name: 'reader', agent: '---\nname: reader\n---\nRead.', tests: 'Checks' };
  let created = false; const live: string[] = [];
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: null, localVm: 'idle', model: 'locked' };
    if (command === 'overview_files') return { state: 'complete', runActive: false, historyState: 'complete', runAgents: [], files: created ? [{ documentId: id, name: 'reader', state: 'readable', source: { agent: 'a'.repeat(64), tests: 'b'.repeat(64) }, summary: { name: 'reader', description: null, model: 'fixture-model', tools: [] }, reason: null, localResult: null }] : [] };
    if (command === 'overview_agents' || command === 'overview_places') live.push(command);
    if (command === 'agent_open') { created = true; return pair; }
    if (command === 'agent_run_status') return { documentId: id, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    throw { kind: 'unavailable' };
  });
  render(<App />);
  await within(screen.getByRole('navigation')).findByText('No saved agents yet');
  await userEvent.click(screen.getByRole('button', { name: 'Designer' }));
  await screen.findByRole('heading', { name: 'reader', level: 1 });
  await within(screen.getByRole('navigation')).findByText('1 saved agent');
  expect(live).toEqual([]);
});

test('Designer opens the owned agent without workspace setup and shares its saved state', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => command === 'setup_status' ? { selected: null, localVm: 'idle', model: 'locked' }
    : command === 'agent_open' ? { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: '---\nname: web-reader\ndescription: Reads a page.\n---\nRead.', tests: 'Checks' }
    : command === 'agent_run_status' ? { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null }
    : { kind: 'version', version: 'v0.4.2', hasDiagnostics: false });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect(await screen.findByRole('heading', { name: 'web-reader', level: 1 })).toBeTruthy();
  expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
  expect(screen.getByRole('main', { name: 'web-reader' })).toBeTruthy();
  expect(within(screen.getByRole('contentinfo')).queryByText('Saved')).toBeNull();
});

test.each([
  ['idle', null, 'Choose where agents run', 'Choose where agents run'],
  ['preparing', 'localVm', 'Getting your workspace ready, step 2 of 4', 'Setting up, step 2 of 4'],
  ['installing', 'localVm', 'Getting your workspace ready, step 2 of 4', 'Setting up, step 2 of 4'],
  ['starting', 'localVm', 'Getting your workspace ready, step 2 of 4', 'Setting up, step 2 of 4'],
  ['ready', 'localVm', 'AI not connected', 'AI not connected'],
  ['failed', 'localVm', "Your workspace didn't start. Your agents are safe.", 'Setup failed'],
  ['closed', 'localVm', 'Workspace stopped', 'Workspace stopped'],
  ['notIntegrated', null, 'Workspace unavailable', 'Workspace unavailable'],
])('chrome reflects %s without invented readiness', async (localVm, selected, detail, tabLine) => {
  mockIPC(command => command === 'workspace_upgrade_status' ? noUpgrade : command === 'setup_status' ? { selected, localVm, model: 'locked' } : { kind: 'version', version: 'v0.4.2', hasDiagnostics: false });
  vi.stubGlobal('isTauri', true);
  render(<App />);
  const footer = screen.getByRole('contentinfo');
  expect(await within(footer).findByText(detail)).toBeTruthy();
  expect(within(screen.getByRole('navigation')).getByText(tabLine)).toBeTruthy();
  expect(within(footer).queryByText(/saved|GPT|tests passing|\d+ of \d+ passed/i)).toBeNull();
});

test('unbuilt controls are disabled with discoverable reasons', async () => {
  const user = userEvent.setup(); render(<App />);
  expect(screen.queryByRole('button', { name: /Search/ })).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect(screen.getByRole('button', { name: 'New agent' }).hasAttribute('disabled')).toBe(false);
  expect(screen.getByRole('button', { name: 'Send to a cluster' }).title).toMatch(/agent folder|Reopen your agent/);
  // Browser preview cannot read native selection, so the host is neither "This computer" nor "No cluster selected".
  expect(within(screen.getByRole('banner')).getByText('Status unavailable')).toBeTruthy();
  // The Overview page also has a "This computer" heading, so assert on the host chip.
  expect(document.querySelector('.host-chip')?.textContent).not.toContain('This computer');
});

test('setup refreshes while another page is open', async () => {
  let localVm = 'ready';
  mockIPC(command => command === 'workspace_upgrade_status' ? noUpgrade : command === 'setup_status' ? { selected: 'localVm', localVm, model: localVm === 'ready' ? 'available' : 'locked' } : { kind: 'version', version: 'v0.4.2', hasDiagnostics: false });
  vi.stubGlobal('isTauri', true);
  const user = userEvent.setup(); render(<App />);
  await within(screen.getByRole('contentinfo')).findByText('Connect the AI, step 3 of 4');
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  localVm = 'failed';
  expect(await within(screen.getByRole('navigation')).findByText('Setup failed', {}, { timeout: 2000 })).toBeTruthy();
  const footer = within(screen.getByRole('contentinfo'));
  expect(footer.getByText("Your workspace didn't start. Your agents are safe.")).toBeTruthy();
  expect(footer.queryByText('Setup failed')).toBeNull();
  expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
});

test('model acknowledgment and observed check counts survive navigation', async () => {
  const pair = { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: '---\nname: page-reader\n---\nRead a page.', tests: 'A check' };
  const snapshot = { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'complete', canRun: true, report: { verdict: 'failed', cases: [
    { id: 'a', question: 'What did the page say?', phase: 'Succeeded', answer: 'ok', completedTools: [], assertions: [{ id: 'a1', kind: 'contains', expected: 'checksum', verdict: 'passed' }], verdict: 'passed' },
    { id: 'b', question: 'What did the page say?', phase: 'Succeeded', answer: 'no', completedTools: [], assertions: [{ id: 'b1', kind: 'contains', expected: 'checksum', verdict: 'failed' }], verdict: 'failed' },
  ] }, acknowledged: [], pending: null, error: null };
  let modelConnected = false;
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_connect_model') { modelConnected = true; return { ready: true }; }
    if (command === 'agent_open') return pair;
    if (command === 'agent_run_status') return { ...snapshot, modelConnected };
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  vi.stubGlobal('isTauri', true);
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  expect(await within(screen.getByRole('contentinfo')).findByText('Workspace ready')).toBeTruthy();
  expect(document.querySelector('.host-chip')?.getAttribute('title')).toBe('The workspace is ready on this computer.');
  expect(document.querySelector('.host-chip')?.textContent).toBe('This computer: The workspace is ready on this computer.');
  expect(document.querySelector('.host-chip .live-dot')?.className).toContain('ready');
  await user.click(screen.getByRole('button', { name: 'Read its instructions' }));
  expect(await within(screen.getByRole('contentinfo')).findByText('1 of 2 passed')).toBeTruthy();
  expect(screen.queryByText(/Connect the AI in Clusters first\./)).toBeNull();
  expect(within(screen.getByRole('navigation')).getByText((_, node) => node?.tagName === 'SMALL' && node.textContent === 'page-reader, 1 test failing')).toBeTruthy();
  expect(document.querySelector('.tab-status-word')?.textContent).toBe('failing');
  expect(within(screen.getByRole('navigation')).getByText('Its test failed')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(within(screen.getByRole('contentinfo')).getByText('Its test failed')).toBeTruthy();
  expect(within(screen.getByRole('region', { name: 'Connect the AI' })).getByText('Connected')).toBeTruthy();
  expect(screen.queryByRole('button', { name: /Model connected|Connecting/ })).toBeNull();
});

test.each(['cancelling', 'cancelled', 'beforeStart', 'unconfirmed'] as const)('a %s run never announces tests passing', async cancellation => {
  const empty = cancellation !== 'cancelled';
  const snapshot = { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: cancellation === 'cancelling' ? 'running' : 'complete', canRun: cancellation !== 'cancelling', cancellation,
    report: { verdict: cancellation === 'unconfirmed' ? 'unknown' : 'cancelled', cases: empty ? [] : [{ id: 'one', question: 'Question', phase: 'Cancelled', answer: null, completedTools: [], assertions: [], verdict: 'cancelled' }] }, acknowledged: [], pending: null, error: null };
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'locked' };
    if (command === 'agent_open') return { documentId: snapshot.documentId, name: 'reader', agent: '---\nname: reader\n---\nRead.', tests: 'Check' };
    if (command === 'agent_run_status') return snapshot;
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  vi.stubGlobal('isTauri', true);
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  const label = cancellation === 'unconfirmed' ? 'cancellation not confirmed' : cancellation === 'cancelling' ? 'cancelling tests' : 'tests cancelled';
  const tab = await within(screen.getByRole('navigation')).findByTitle(`reader, ${label}`);
  if (cancellation === 'unconfirmed') expect(tab.querySelector('.tab-status-word')?.className).toContain('status-needs-you');
  expect(within(screen.getByRole('contentinfo')).getByText(label[0].toUpperCase() + label.slice(1))).toBeTruthy();
  expect(within(screen.getByRole('contentinfo')).queryByText(/tests passing|\d+ of \d+ passed/i)).toBeNull();
});

test('unconfirmed run cancellation offers native Quit Bridge without another workspace admission', async () => {
  vi.stubGlobal('isTauri', true);
  const documentId = '9b2b93c4-375c-4815-9717-39852a96e411';
  const ipc = vi.fn(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'locked' };
    if (command === 'quit_bridge') return Promise.reject(Error('Quit unavailable'));
    if (command === 'agent_open') return { documentId, name: 'reader', agent: '---\nname: reader\n---\nRead.', tests: 'Check' };
    if (command === 'agent_run_status') return { documentId, busy: null, state: 'complete', canRun: true, cancellation: 'unconfirmed', canRetryCancellation: false, canCheckCancellation: false, report: { verdict: 'unknown', cases: [] }, acknowledged: [], pending: null, error: 'session' };
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  }); mockIPC(ipc);
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await user.click(await screen.findByRole('button', { name: 'Quit Bridge' }));
  expect(await screen.findByText("Couldn't quit Bridge. Use Quit Bridge from its menu.")).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Designer' }).getAttribute('aria-current')).toBe('page');
  expect(screen.queryByRole('button', { name: 'Stop the workspace' })).toBeNull();
  expect(localStorage.getItem('bridge.firstTest.v1')).toBeNull();
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'quit_bridge')).toHaveLength(1);
  expect(ipc.mock.calls.some(([cmd]) => ['setup_cancel_local', 'setup_select_local', 'setup_change_local', 'agent_run'].includes(cmd))).toBe(false);
});

test('a running check finishes in the footer after leaving Designer', async () => {
  let complete = false;
  const report = { verdict: 'passed', cases: [{ id: 'a', question: 'What did the page say?', phase: 'Succeeded', answer: 'ok', completedTools: [], assertions: [{ id: 'a1', kind: 'contains', expected: 'checksum', verdict: 'passed' }], verdict: 'passed' }] };
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'locked' };
    if (command === 'agent_open') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: '---\nname: reader\n---\nRead.', tests: 'Check' };
    if (command === 'agent_run_status') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: complete ? 'complete' : 'running', canRun: complete, report: complete ? report : null, acknowledged: [], pending: null, error: null };
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  vi.stubGlobal('isTauri', true);
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await within(screen.getByRole('contentinfo')).findByText('Running tests…');
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  complete = true;
  expect(await within(screen.getByRole('contentinfo')).findByText('1 of 1 passed')).toBeTruthy();
  expect(document.querySelectorAll('#page-title')).toHaveLength(1);
});

test('the idle refresh does not erase a delayed selection failure', async () => {
  let rejectSelection!: (error: unknown) => void;
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: null, localVm: 'idle', model: 'locked' };
    if (command === 'setup_select_local') return new Promise((_, reject) => { rejectSelection = reject; });
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  vi.stubGlobal('isTauri', true);
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Continue' }));
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 1200)); });
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(true);
  await act(async () => { rejectSelection(new Error('selection write failed')); });
  expect(await screen.findByText('The setup choice could not be read or saved.')).toBeTruthy();
});

test('an unavailable example is not reported as still opening', async () => {
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'locked' };
    if (command === 'agent_open') return Promise.reject(new Error('file unavailable'));
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  vi.stubGlobal('isTauri', true);
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  expect(await within(screen.getByRole('navigation')).findByText('Example unavailable')).toBeTruthy();
  expect(within(screen.getByRole('navigation')).queryByText('Opening example…')).toBeNull();
});

test.each(['unknown', 'unavailable'])('background check status preserves %s outcomes', async state => {
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'agent_open') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: '---\nname: reader\n---\nRead.', tests: 'Check' };
    if (command === 'agent_run_status') return state === 'unavailable' ? Promise.reject(new Error('IPC failed'))
      : { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'unknown', canRun: false, report: null, acknowledged: [], pending: { kind: 'Task', name: 'maybe-created' }, error: 'observe' };
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  vi.stubGlobal('isTauri', true);
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  const expected = state === 'unknown' ? 'Test result unknown' : 'Test status unavailable';
  await within(screen.getByRole('contentinfo')).findByText(expected);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  expect(within(screen.getByRole('contentinfo')).getByText(expected)).toBeTruthy();
  expect(within(screen.getByRole('navigation')).getByText((_, node) => node?.tagName === 'SMALL' && node.textContent === `reader, ${expected.toLowerCase()}`)).toBeTruthy();
  expect(within(screen.getByRole('contentinfo')).getByRole('status').textContent).toContain(expected);
});

test('a retained native report for older source is stale in the shared chrome', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => command === 'setup_status' ? { selected: null, localVm: 'idle', model: 'locked' }
    : command === 'agent_open' ? { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: '---\nname: reader\n---\nEdited file.', tests: 'Checks', source: { agent: 'b'.repeat(64), tests: 'c'.repeat(64) } }
    : { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'complete', canRun: false, source: { agent: 'a'.repeat(64), tests: 'c'.repeat(64) }, report: { verdict: 'passed', cases: [{ id: 'check', question: 'What did the page say?', phase: 'Succeeded', answer: 'Earlier answer', completedTools: [], assertions: [{ id: 'contains-1', kind: 'contains', expected: 'checksum', verdict: 'passed' }], verdict: 'passed' }] }, acknowledged: [], pending: null, error: null });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect(await within(screen.getByRole('contentinfo')).findByText('Tests not run since your edit')).toBeTruthy();
  expect(within(screen.getByRole('contentinfo')).queryByText('1 of 1 passed')).toBeNull();
});
test('Open example retries a failed read without discarding successful runs', async () => {
  let fail = true;
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'agent_open') return fail ? Promise.reject(new Error('read failed')) : { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: '---\nname: recovered-reader\n---\nRead.', tests: 'Check' };
    if (command === 'agent_run_status') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  vi.stubGlobal('isTauri', true);
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await within(screen.getByRole('navigation')).findByText('Example unavailable');
  await user.click(screen.getByRole('button', { name: 'Clusters' })); fail = false;
  await user.click(screen.getByRole('button', { name: 'Read its instructions' }));
  expect(await within(screen.getByRole('navigation')).findByText((_, node) => node?.tagName === 'SMALL' && node.textContent === 'recovered-reader, ready to edit')).toBeTruthy();
});

test('a transient setup read failure recovers while off Clusters', async () => {
  let fail = true;
  mockIPC(command => command === 'workspace_upgrade_status' ? noUpgrade : command === 'setup_status' ? fail ? Promise.reject(new Error('read failed'))
    : { selected: 'localVm', localVm: 'starting', model: 'locked' }
    : { kind: 'version', version: 'v0.4.2', hasDiagnostics: false });
  vi.stubGlobal('isTauri', true);
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await within(screen.getByRole('navigation')).findByText('Workspace unavailable');
  await user.click(screen.getByRole('button', { name: 'Overview' })); fail = false;
  expect(await within(screen.getByRole('navigation')).findByText('Setting up, step 2 of 4', {}, { timeout: 2000 })).toBeTruthy();
});

test('a transient setup read failure does not invalidate an admitted model connection', async () => {
  let fail = false;
  let finishConnect!: (value: unknown) => void;
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return fail ? Promise.reject(new Error('read failed')) : { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_connect_model') return new Promise(resolve => { finishConnect = resolve; });
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  vi.stubGlobal('isTauri', true);
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  fail = true;
  await within(screen.getByRole('navigation')).findByText('Workspace unavailable', {}, { timeout: 2000 });
  await act(async () => { finishConnect({ ready: true }); });
  expect(within(screen.getByRole('region', { name: 'Connect the AI' })).getByText('Connected')).toBeTruthy();
  fail = false;
  expect(await within(screen.getByRole('contentinfo')).findByText('Workspace ready', {}, { timeout: 2000 })).toBeTruthy();
  expect(within(screen.getByRole('region', { name: 'Connect the AI' })).getByText('Connected')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Model connected' })).toBeNull();
});

test.each(['saved file', 'cluster Agent'])('%s Open keeps the selected cluster and shows local tests beside Send, without a second Use or a Send', async origin => {
  vi.stubGlobal('isTauri', true);
  const id = '12345678-1234-1234-1234-123456789abc';
  const context = { id: '00000000-0000-4000-8000-000000000001', name: 'team-dev', current: true };
  const target = { id: '00000000-0000-4000-8000-000000000002', context_id: context.id, context_name: context.name, service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'ghcr.io/orka:0.9.4' };
  const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'team-model', ready: true };
  const connected = { status: 'connected', providers: [provider], failure: null };
  const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
  const calls: string[] = []; const catalogs: unknown[] = []; let uses = 0;
  mockIPC((command, args) => {
    calls.push(command);
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'cluster_catalog') catalogs.push(args);
    if (command === 'workspace_catalog' || command === 'cluster_catalog') return { defaultModel: 'literal-model', models: { state: 'ready', items: [{ id: 'literal-model' }] }, tools: { state: 'ready', items: [{ kind: 'builtin', name: 'web_fetch', description: 'Read a page' }] } };
    if (command === 'cluster_contexts') return [context];
    if (command === 'cluster_discover') return [target];
    if (command === 'cluster_use') return ++uses === 1 ? connected : connected;
    if (command === 'cluster_select') return { target, providers: [provider] };
    if (command === 'overview_places') return { historyState: 'complete', unobserved: [], places: origin === 'saved file' ? [] : [{ key: 'team-place', context: 'team-dev', service: 'control/orka', namespace: 'agents', stale: false, observedAt: 123, agents: [{ name: 'bridge-reader', namespace: 'agents', uid: 'remote-uid', agentType: null, model: null, tools: null, record: { id, source, targetDisplay: 'team-dev · agents', targetUids: { serviceNamespace: 'control', serviceUid: 'svc', deploymentNamespace: 'control', deploymentUid: 'dep', watchedNamespace: 'agents' }, provider: 'team-model', plannedAgentName: 'bridge-reader', agentUid: 'remote-uid', verdict: 'passed', time: 123 } }] }] };
    if (command === 'overview_files') return { state: 'complete', runActive: false, files: [{ documentId: id, name: 'reader', source, state: 'readable', summary: null, reason: null }] };
    if (command === 'overview_agents') return { state: 'unavailable', installation: null, agents: [], observation: null, observedAt: null, runActive: false, reason: 'stopped' };
    if (command === 'agent_open') return { documentId: id, name: 'reader', agent: '---\nname: reader\nmodel: literal-model\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools: []\n---\nRead.', tests: 'Checks', source };
    if (command === 'agent_run_status') return { documentId: id, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    if (command === 'send_status') return { documentId: id, view: null, busy: null };
    if (command === 'send_history') return [];
    if (command === 'engine_call') return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
    return null;
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await user.click(await screen.findByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  await user.click(await screen.findByRole('button', { name: 'kubectl context' }));
  await user.click(screen.getByRole('button', { name: context.name }));
  await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  await screen.findByRole('button', { name: 'Open your agent' });
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  const originRow = await screen.findByRole('article', { name: 'reader' });
  await user.click(within(originRow).getByRole('button', { name: 'Open reader' }));
  await screen.findByRole('heading', { name: 'reader', level: 1 });
  // Opening a file or its agent from Overview keeps the selected Send destination; local tests are independent of it.
  expect(document.querySelector('.host-chip')?.textContent).toContain('team-dev');
  expect(await screen.findByRole('region', { name: 'Team cluster' })).toBeTruthy();
  expect(screen.getByRole('region', { name: 'This computer' })).toBeTruthy();
  await waitFor(() => expect((screen.getByRole('button', { name: 'Add a tool' }) as HTMLButtonElement).disabled).toBe(false));
  expect(catalogs).toContainEqual({ targetId: target.id, providerId: provider.id });
  expect(uses).toBe(1); expect(calls).not.toContain('send_start');
});

test('setup Open example restores the sample after Overview opened another document', async () => {
  vi.stubGlobal('isTauri', true);
  const sample = '9b2b93c4-375c-4815-9717-39852a96e411'; const other = '12345678-1234-1234-1234-123456789abc';
  mockIPC((command, args) => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'locked' };
    if (command === 'overview_files') return { state: 'complete', runActive: false, files: [{ documentId: other, name: 'other', state: 'readable', source: null, summary: null, reason: null }] };
    if (command === 'agent_open') { const id = (args as { documentId: string }).documentId; const name = id === sample ? 'sample' : 'other'; return { documentId: id, name, agent: `---\nname: ${name}\n---\nRead.`, tests: 'Checks' }; }
    if (command === 'agent_run_status') return { documentId: (args as { documentId: string }).documentId, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open other' }));
  await screen.findByRole('heading', { name: 'other', level: 1 });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Read its instructions' }));
  expect(await screen.findByRole('heading', { name: 'sample', level: 1 })).toBeTruthy();
});

test('Overview Open retries a transient failure for the same document', async () => {
  vi.stubGlobal('isTauri', true); let opens = 0;
  const id = '9b2b93c4-375c-4815-9717-39852a96e411';
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'locked' };
    if (command === 'overview_files') return { state: 'complete', runActive: false, files: [{ documentId: id, name: 'reader', source: null, state: 'readable', summary: null, reason: null }] };
    if (command === 'agent_open') { if (++opens === 1) throw Error('Temporary read failure'); return { documentId: id, name: 'reader', agent: '---\nname: reader\n---\nRead.', tests: 'Checks' }; }
    if (command === 'agent_run_status') return { documentId: id, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open reader' }));
  await within(screen.getByRole('navigation')).findByText('Example unavailable');
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open reader' }));
  expect(await screen.findByRole('heading', { name: 'reader', level: 1 })).toBeTruthy(); expect(opens).toBe(2);
});

test('opening the already-local file does not acquire another VM catalog session', async () => {
  vi.stubGlobal('isTauri', true); let catalogs = 0;
  const id = '9b2b93c4-375c-4815-9717-39852a96e411';
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'workspace_catalog') { catalogs++; return null; }
    if (command === 'overview_files') return { state: 'complete', runActive: false, files: [{ documentId: id, name: 'reader', source: null, state: 'readable', summary: null, reason: null }] };
    if (command === 'overview_agents') return { state: 'unavailable', installation: null, agents: [], observation: null, observedAt: null, runActive: false, reason: 'stopped' };
    if (command === 'agent_open') return { documentId: id, name: 'reader', agent: '---\nname: reader\n---\nRead.', tests: 'Checks' };
    if (command === 'agent_run_status') return { documentId: id, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await screen.findByRole('button', { name: 'Refresh tools and models' });
  const before = catalogs;
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open reader' }));
  await screen.findByRole('heading', { name: 'reader', level: 1 });
  expect(catalogs).toBe(before);
});

test('Overview waits for native busy release even when the report is already complete', async () => {
  vi.stubGlobal('isTauri', true);
  const id = '9b2b93c4-375c-4815-9717-39852a96e411';
  let busy = true; let lists = 0;
  mockIPC(command => {
    if (command === 'workspace_upgrade_status') return noUpgrade;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'locked' };
    if (command === 'agent_open') return { documentId: id, name: 'reader', agent: '---\nname: reader\n---\nRead.', tests: 'Checks' };
    if (command === 'agent_run_status') return { documentId: id, busy: busy ? { documentId: id, name: 'reader' } : null, state: 'complete', canRun: false, report: { verdict: 'passed', cases: [{ id: 'a', question: 'What did the page say?', phase: 'Succeeded', answer: 'ok', completedTools: [], assertions: [{ id: 'a1', kind: 'contains', expected: 'ok', verdict: 'passed' }], verdict: 'passed' }] }, acknowledged: [], pending: null, error: null };
    if (command === 'overview_files') return { state: 'complete', files: [], runActive: busy };
    if (command === 'overview_agents') { lists++; return { state: 'unavailable', installation: null, agents: [], observation: null, observedAt: null, runActive: false, reason: 'stopped' }; }
    return { kind: 'version', version: 'v0.4.2', hasDiagnostics: false };
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await within(screen.getByRole('contentinfo')).findByText('1 of 1 passed');
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await screen.findByText('Updating when the test finishes.');
  expect(lists).toBe(0);
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  busy = false;
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await screen.findByText('The workspace is stopped. Start it to test or chat.', {}, { timeout: 2000 });
  expect(lists).toBe(1);
});

test('Clusters reads setup without engine diagnostics or legacy IPC', async () => {
  const ipc = vi.fn(command => command === 'setup_status'
    ? { selected: null, localVm: 'notIntegrated', model: 'locked' }
    : { kind: 'version', version: 'v0.4.2', hasDiagnostics: false });
  mockIPC(ipc);
  vi.stubGlobal('isTauri', true);
  const fetch = vi.fn(); vi.stubGlobal('fetch', fetch);
  const user = userEvent.setup(); render(<App />);
  expect(await screen.findByRole('button', { name: 'Set up' })).toBeTruthy();
  expect(screen.queryByText('Engine diagnostics')).toBeNull();
  expect(ipc.mock.calls.every(([command]) => ['setup_requirements', 'setup_status', 'workspace_upgrade_status', 'cluster_places', 'overview_files'].includes(command))).toBe(true);
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_requirements')).toEqual([['setup_requirements', {}]]);
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect(screen.queryByRole('heading', { name: 'Engine available' })).toBeNull();
  expect(ipc.mock.calls.filter(call => call[0] === 'engine_call')).toHaveLength(0);
  expect(fetch).not.toHaveBeenCalled();
});

test('Overview Refresh retries a failed native lookup before refreshing its recovered destination', async () => {
  vi.stubGlobal('isTauri', true);
  const id = '9b2b93c4-375c-4815-9717-39852a96e411';
  const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
  let lookups = 0;
  const placeReads: unknown[] = [];
  mockIPC((command, args) => {
    if (command === 'setup_status') return { selected: null, localVm: 'notIntegrated', model: 'locked' };
    if (command === 'agent_open') return { documentId: id, name: 'reader', agent: '---\nname: reader\n---\nRead.', tests: 'Checks', source };
    if (command === 'agent_run_status') return { documentId: id, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null, busy: null };
    if (command === 'send_status') return { documentId: id, busy: null, view: { documentId: id, id: '00000000-0000-4000-8000-000000000004', targetId: '00000000-0000-4000-8000-000000000002', providerId: '00000000-0000-4000-8000-000000000003', targetName: 'team', source, model: 'm', agentName: 'bridge-run-one', state: 'unknown', stage: 'stopped', report: null, acknowledged: [], pending: null, error: 'stopped', storageError: false, agentAction: 'update' } };
    if (command === 'send_history') return [];
    if (command === 'send_agent_focus') return ++lookups === 1 ? Promise.reject(new Error('Transient history read failure')) : { key: 'team', name: 'bridge-run-one', namespace: 'agents', uid: 'uid-one' };
    if (command === 'send_agent_check') return 'present';
    if (command === 'overview_places') { placeReads.push(args); return { places: [], historyState: 'complete', unobserved: [] }; }
    return null;
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  await user.click(await screen.findByRole('button', { name: 'See in Overview' }));
  const note = await screen.findByText('Cannot check this agent. Your saved result is unchanged.');
  placeReads.length = 0;
  await user.click(within(note.parentElement!).getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(lookups).toBe(2));
  await waitFor(() => expect(placeReads).toEqual([{ key: 'team' }]));
  await screen.findByText('Not shown in Overview yet.');
  expect(screen.queryByText('Cannot check this agent. Your saved result is unchanged.')).toBeNull();
});

test.each([true, false])('pending See in Overview skips unrelated reads and respects later navigation (leave: %s)', async leave => {
  vi.stubGlobal('isTauri', true);
  const id = '9b2b93c4-375c-4815-9717-39852a96e411';
  const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
  let release!: (value: unknown) => void;
  const placeReads: unknown[] = [];
  mockIPC((command, args) => {
    if (command === 'setup_status') return { selected: null, localVm: 'notIntegrated', model: 'locked' };
    if (command === 'agent_open') return { documentId: id, name: 'reader', agent: '---\nname: reader\n---\nRead.', tests: 'Checks', source };
    if (command === 'agent_run_status') return { documentId: id, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null, busy: null };
    if (command === 'send_status') return { documentId: id, busy: null, view: { documentId: id, id: '00000000-0000-4000-8000-000000000004', targetId: '00000000-0000-4000-8000-000000000002', providerId: '00000000-0000-4000-8000-000000000003', targetName: 'team', source, model: 'm', agentName: 'bridge-run-one', state: 'unknown', stage: 'stopped', report: null, acknowledged: [], pending: null, error: 'stopped', storageError: false, agentAction: 'update' } };
    if (command === 'send_history') return [];
    if (command === 'send_agent_focus') return new Promise(resolve => { release = resolve; });
    if (command === 'overview_places') { placeReads.push(args); return { places: [], historyState: 'complete', unobserved: [] }; }
    return null;
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  await user.click(await screen.findByRole('button', { name: 'See in Overview' }));
  await waitFor(() => expect(release).toBeDefined());
  expect(placeReads).toEqual([]);
  if (leave) await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await act(async () => release({ key: 'team', name: 'bridge-run-one', namespace: 'agents', uid: 'uid-one' }));
  if (!leave) await waitFor(() => expect(placeReads).toEqual([{ key: 'team' }]));
  expect(screen.getByRole('button', { name: leave ? 'Clusters' : 'Overview' }).getAttribute('aria-current')).toBe('page');
  if (leave) expect(placeReads).toEqual([]);
});
