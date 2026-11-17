import { openLocalSetup } from './test-navigation';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../App';

const sample = '9b2b93c4-375c-4815-9717-39852a96e411';
const pair = { agent: '---\nname: web-reader\n---\nRead a page.', tests: 'A checksum test.', source: { agent: 'a'.repeat(64), tests: 'b'.repeat(64) } };
const other = '12345678-1234-1234-1234-123456789abc';
const idle = { documentId: sample, busy: null, state: 'idle', canRun: true, modelConnected: true, report: null, acknowledged: [], pending: null, error: null };
function native(holdSave = false, lostReplies = false) {
  vi.stubGlobal('isTauri', true);
  let selected = false; let running = false; let finished = false; let stopped = false;
  let attemptId: unknown;
  let unreadable = false; let unreadableFiles = false; let verdict = 'passed'; let saved = pair; let modelUnavailable = false;
  const status = () => ({ selected: selected ? 'localVm' : null, localVm: stopped ? 'cancelled' : selected ? 'ready' : 'idle', model: selected && !stopped ? 'available' : 'locked' });
  const ipc = vi.fn((command: string, payload?: unknown) => {
    const args = payload as Record<string, unknown> | undefined;
    if (command === 'setup_status') return status();
    if (command === 'setup_requirements') return null;
    if (command === 'setup_select_local') { selected = true; stopped = false; if (lostReplies) throw Error('Lost selection reply'); return status(); }
    if (command === 'setup_cancel_local') { stopped = true; running = false; return status(); }
    if (command === 'setup_change_local') { selected = false; return status(); }
    if (command === 'setup_example') return { agent: pair.agent, tests: pair.tests };
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'setup_remove_model') return { ready: false };
    if (command === 'workspace_catalog') return { defaultModel: 'file-model', models: { state: 'ready', items: [{ id: 'file-model' }] }, tools: { state: 'ready', items: [] } };
    if (command === 'agent_select') return null;
    if (command === 'agent_open') { if (unreadableFiles) throw Error('Lost file read'); return args?.documentId === other ? { ...pair, documentId: other, name: 'other', agent: '---\nname: other\n---\nOther instructions.' } : { ...saved, documentId: sample, name: 'web-reader' }; }
    if (command === 'agent_save') { if (holdSave) return new Promise(() => {}); saved = { ...pair, agent: String(args?.content), source: { ...pair.source, agent: 'c'.repeat(64) } }; return { ...saved, documentId: sample, name: 'web-reader' }; }
    if (command === 'agent_run') { attemptId = args?.attemptId; running = true; finished = false; if (lostReplies) throw Error('Lost run reply'); return { ...idle, ...(attemptId ? { attemptId } : {}), state: 'running', canRun: false, busy: { documentId: sample, name: 'web-reader' } }; }
    if (command === 'agent_run_status') {
      if (args?.documentId === other) return { ...idle, documentId: other };
      if (unreadable) throw Error('Lost sample status');
      if (modelUnavailable) return { ...idle, state: 'unknown', error: 'model' };
      return finished ? { ...idle, ...(attemptId ? { attemptId } : {}), source: pair.source, state: 'complete', report: { verdict, cases: [{ id: 'checksum', question: 'What checksum is on the page?', phase: 'Succeeded', answer: 'Correct checksum', completedTools: ['web_fetch'], assertions: [{ id: 'checksum', kind: 'contains', expected: 'Correct checksum', verdict }], verdict }] } } : running ? { ...idle, state: 'running', canRun: false, busy: { documentId: sample, name: 'web-reader' } } : idle;
    }
    if (command === 'send_history' || command === 'cluster_contexts') return [];
    if (command === 'overview_files') return { state: 'complete', files: [{ documentId: other, name: 'other', state: 'readable', source: null, summary: null, reason: null }], runActive: running && !finished };
    if (command === 'overview_agents') return { state: 'unavailable', installation: null, agents: [], observation: null, observedAt: null, runActive: false, reason: 'stopped' };
    throw Error('Unexpected command: ' + command);
  });
  mockIPC(ipc); return { ipc, unavailable: () => { modelUnavailable = true; }, finish: () => { finished = true; }, loseWorkspace: () => { selected = false; }, loseRunStatus: () => { unreadable = true; }, restoreRunStatus: () => { unreadable = false; }, loseFileReads: () => { unreadableFiles = true; }, restoreFileReads: () => { unreadableFiles = false; }, editFiles: () => { saved = { ...pair, agent: pair.agent + ' External edit.', source: { ...pair.source, agent: 'd'.repeat(64) } }; }, failNext: () => { verdict = 'failed'; } };
}
beforeEach(() => localStorage.setItem('bridge.welcomeSeen.v1', '1'));
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

async function completeSetup() {
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Continue' }));
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }, { timeout: 2000 }));
  return user;
}

test('first setup stays for its first-pass story, opens only on request and never replays the moment or run', async () => {
  const { ipc, finish } = native(); const user = await completeSetup();
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1));
  expect(screen.getByRole('button', { name: 'Clusters' }).getAttribute('aria-current')).toBe('page');
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe('Run your first agentRunning');
  expect(screen.getByText('Setting up, step 4 of 4')).toBeTruthy();
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.getAttribute('aria-current')).toBe('step');
  expect(screen.queryByRole('button', { name: 'See what it answered' })).toBeNull();
  finish();
  expect(await screen.findByRole('heading', { name: 'Your first agent passed its test' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Clusters' }).getAttribute('aria-current')).toBe('page');
  expect(screen.getByText('Correct checksum', { selector: 'p' })).toBeTruthy();
  expect(screen.getByText('Read a web page', { exact: false })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Open web-reader' }));
  expect(screen.getByRole('button', { name: 'Designer' }).getAttribute('aria-current')).toBe('page');
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toEqual([['agent_run', { documentId: sample, bundledExample: true, attemptId: expect.any(String) }]]);
});

test('opening Designer during the automatic first test defers catalog discovery until it finishes', async () => {
  const { ipc, finish } = native(); const user = await completeSetup();
  await screen.findByText('Setting up, step 4 of 4');
  await user.click(screen.getByRole('button', { name: 'Read its instructions' }));
  await screen.findByRole('textbox', { name: 'Instructions' });
  expect(ipc.mock.calls.filter(([command]) => command === 'workspace_catalog')).toHaveLength(0);
  expect(screen.getAllByText('Tools and models are paused while another operation finishes.').length).toBeGreaterThan(0);
  expect(screen.queryByText(/Couldn't read (tools|models)/)).toBeNull();
  finish();
  await waitFor(() => expect(ipc.mock.calls.some(([command]) => command === 'workspace_catalog')).toBe(true));
  expect(screen.queryByRole('button', { name: 'Refresh tools and models' })).toBeNull();
  expect(ipc.mock.calls.filter(([command]) => command === 'agent_run')).toHaveLength(1);
  expect(localStorage.getItem('bridge.firstTest.v1')).toBe('attempted');
});

test('Stop after the first-pass story retains the workspace without replaying the story or automatic run on restart', async () => {
  const { ipc, finish } = native(); const user = await completeSetup();
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1)); finish();
  await user.click(await screen.findByRole('button', { name: 'Open web-reader' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(await screen.findByRole('button', { name: 'Stop the workspace' }));
  await user.click(screen.getByRole('button', { name: 'Stop workspace' }));
  await user.click(await screen.findByRole('button', { name: 'Start' }));
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await screen.findByRole('button', { name: 'Change' });
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('This computer');
  expect(screen.queryByRole('heading', { name: 'Your first agent passed its test' })).toBeNull();
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1);
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_change_local')).toHaveLength(0);
});

test('reading instructions before completion does not dismiss the later payoff', async () => {
  const { finish } = native(); const user = await completeSetup();
  await screen.findByText('Setting up, step 4 of 4');
  await user.click(screen.getByRole('button', { name: 'Read its instructions' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' })); finish();
  expect(await screen.findByRole('heading', { name: 'Your first agent passed its test' })).toBeTruthy();
});

test('a shown payoff cannot replay after its evidence becomes unavailable and recovers', async () => {
  const { ipc, finish, loseRunStatus, restoreRunStatus } = native(); await completeSetup();
  await waitFor(() => expect(ipc.mock.calls.filter(([command]) => command === 'agent_run')).toHaveLength(1));
  finish();
  await screen.findByRole('heading', { name: 'Your first agent passed its test' });
  loseRunStatus(); await screen.findByRole('button', { name: 'Check again' });
  restoreRunStatus(); await screen.findByRole('button', { name: 'See what it answered' });
  expect(screen.queryByRole('heading', { name: 'Your first agent passed its test' })).toBeNull();
});

test('an unreachable model stays in setup with explicit recovery and never replays automatically', async () => {
  const { ipc, unavailable } = native(); const user = await completeSetup();
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1));
  unavailable();
  await screen.findByText('The first test could not be confirmed. If the AI could not be reached, check your connection or choose another provider.');
  expect(screen.queryByRole('heading', { name: 'Your first agent passed its test' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Clusters' }).getAttribute('aria-current')).toBe('page');
  expect(screen.getByRole('button', { name: 'Try again in Designer' })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Choose another provider' }));
  await user.click(await screen.findByRole('button', { name: 'Remove saved sign-in' }));
  await screen.findByRole('group', { name: 'Remove sign-in confirmation' });
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1);
  expect(localStorage.getItem('bridge.firstTest.v1')).toBe('attempted');
  await user.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  expect(await screen.findByRole('button', { name: 'Read its instructions' })).toBeTruthy();
});

test.each(['edited', 'unreadable'] as const)('completion file freshness survives subsequent native status polls (%s)', async state => {
  const { ipc, finish, editFiles, loseFileReads } = native(); const user = await completeSetup();
  await screen.findByText('Setting up, step 4 of 4');
  if (state === 'edited') editFiles(); else loseFileReads();
  finish();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Designer' }).getAttribute('aria-current')).toBe('page'));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  const reads = ipc.mock.calls.filter(([command]) => command === 'agent_run_status').length;
  await waitFor(() => expect(ipc.mock.calls.filter(([command]) => command === 'agent_run_status').length).toBeGreaterThan(reads));
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe(`Run your first agent${state === 'edited' ? 'Needs running' : 'Result unknown'}`);
  expect(screen.queryByRole('heading', { name: 'Your first agent passed its test' })).toBeNull();
  expect(ipc.mock.calls.filter(([command]) => command === 'agent_run')).toHaveLength(1);
});

test.each(['passed', 'failed', 'unreadable', 'edited'] as const)('Setup retains the latest sample evidence (%s), not the automatic result or another document', async state => {
  const { finish, loseRunStatus, failNext } = native(); const user = await completeSetup();
  await waitFor(() => expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe('Run your first agentRunning')); finish();
  await user.click(await screen.findByRole('button', { name: 'Open web-reader' }));
  await screen.findByText('1 of 1 passed');
  if (state === 'failed') {
    failNext(); await user.click(screen.getByRole('button', { name: 'Run tests' })); finish();
    await screen.findByText('0 of 1 passed');
  } else if (state === 'unreadable') {
    loseRunStatus(); await screen.findByText('Run information is not supported by this build.');
  } else if (state === 'edited') {
    await user.click(screen.getByRole('button', { name: 'File' }));
    await user.type(screen.getByRole('textbox', { name: 'Agent file' }), ' Edited.');
    await screen.findByText('Saved', { selector: 'span' });
  }
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(screen.queryByRole('heading', { name: 'Your first agent passed its test' })).toBeNull();
  if (state === 'unreadable') expect(screen.queryByRole('button', { name: 'Choose another provider' })).toBeNull();
  const expected = { passed: 'Its test passed', failed: 'Its test failed', unreadable: 'Result unknown', edited: 'Needs running' }[state];
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe(`Run your first agent${expected}`);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open other' }));
  await screen.findByRole('heading', { name: 'other', level: 1 });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe(`Run your first agent${expected}`);
  const card = within(screen.getByRole('region', { name: 'Workspace setup' }));
  expect(card.getByText(expected, { selector: '.step-result' })).toBeTruthy();
  expect(screen.queryByText('Setting up, step 5 of 5')).toBeNull();
  if (state === 'failed') {
    expect(card.getByText('Its test failed', { selector: '.step-result' }).closest('li')?.className).toBe('step-failed');
    expect(within(screen.getByRole('navigation')).getByText('Its test failed')).toBeTruthy();
    expect(within(screen.getByRole('contentinfo')).getByText('Its test failed')).toBeTruthy();
    expect(document.querySelector('.host-chip .live-dot')?.className).toContain('ready');
    await user.click(screen.getByRole('button', { name: 'Clusters' }));
    expect(within(screen.getByRole('region', { name: 'This computer' })).getByText('Failed', { selector: '.pill' }).className).toContain('status-failed');
    await openLocalSetup(user);
  }
  if (state === 'passed' || state === 'failed') {
    await user.click(card.getByRole('button', { name: 'See what it answered' }));
    await screen.findByRole('heading', { name: 'web-reader', level: 1 });
  }
});

test.each([false, true])('a manual sample test stays observed across agent switches (blocked return: %s)', async blockedReturn => {
  const { finish, failNext } = native(blockedReturn); const user = await completeSetup();
  await waitFor(() => expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe('Run your first agentRunning')); finish();
  await user.click(await screen.findByRole('button', { name: 'Open web-reader' }));
  await screen.findByText('1 of 1 passed');
  failNext(); await user.click(screen.getByRole('button', { name: 'Run tests' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(screen.queryByText('Setting up, step 5 of 5')).toBeNull();
  expect(screen.getByRole('region', { name: 'Workspace setup' }).textContent).toContain('Running test');
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open other' }));
  await screen.findByRole('heading', { name: 'other', level: 1 });
  if (blockedReturn) {
    await user.click(screen.getByRole('button', { name: 'File' }));
    await user.type(screen.getByRole('textbox', { name: 'Agent file' }), ' Unsaved.');
    await user.click(screen.getByRole('button', { name: 'Clusters' }));
    await user.click(screen.getByRole('button', { name: 'Read its instructions' }));
  }
  finish();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await waitFor(() => expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe('Run your first agentIts test failed'));
});

test('a failed sample reopen cannot resurrect Passed from a still-readable native run report', async () => {
  const { ipc, finish, loseFileReads, restoreFileReads } = native(); const user = await completeSetup();
  await waitFor(() => expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe('Run your first agentRunning')); finish();
  await user.click(await screen.findByRole('button', { name: 'Open web-reader' }));
  await screen.findByText('1 of 1 passed'); loseFileReads();
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open other' }));
  await screen.findByText('Your agent folder could not be opened. Open it again from Designer.');
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Read its instructions' }));
  await screen.findByText('Your agent folder could not be opened. Open it again from Designer.');
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await new Promise(resolve => setTimeout(resolve, 550));
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe('Run your first agentResult unknown');
  expect(screen.queryByRole('button', { name: 'See what it answered' })).toBeNull();
  restoreFileReads();
  await user.click(screen.getByRole('button', { name: 'Check again' }));
  await screen.findByRole('button', { name: 'Run tests' });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await screen.findByRole('button', { name: 'See what it answered' });
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1);
});

test('Check again observes status before any report even while the guarded sample open still fails', async () => {
  const { ipc, loseFileReads } = native();
  localStorage.setItem('bridge.firstTest.v1', 'attempted');
  const user = await completeSetup(); loseFileReads();
  await user.click(screen.getByRole('button', { name: 'Read its instructions' }));
  await screen.findByText('Your agent folder could not be opened. Open it again from Designer.');
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  const before = ipc.mock.calls.filter(([c]) => c === 'agent_run_status').length;
  await user.click(screen.getByRole('button', { name: 'Check again' }));
  await screen.findByText('Your agent folder could not be opened. Open it again from Designer.');
  expect(screen.queryByRole('button', { name: 'Run tests' })).toBeNull();
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run_status').length).toBeGreaterThan(before));
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(0);
});

test('Run it again hands an unsaved sample to the existing disabled Designer Run', async () => {
  const { ipc, finish } = native(true); const user = await completeSetup();
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1)); finish();
  await user.click(await screen.findByRole('button', { name: 'Open web-reader' }));
  await screen.findByText('1 of 1 passed');
  await user.click(screen.getByRole('button', { name: 'File' }));
  await user.type(screen.getByRole('textbox', { name: 'Agent file' }), ' Unsaved.');
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Run it again' }));
  expect(screen.getByRole('button', { name: 'Run tests' }).matches(':disabled')).toBe(true);
  expect(screen.getByRole('button', { name: 'Run tests' }).title).toBe('Save your changes before running the test');
  expect(screen.getByRole('textbox', { name: 'Agent file' }).textContent).toContain('Unsaved.');
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1);
});

test('a newer automatic test replaces retained sample evidence after Designer switches documents', async () => {
  const { ipc, finish } = native(); const user = userEvent.setup(); render(<App />);
  await openLocalSetup(user);
  await user.click(await screen.findByRole('button', { name: 'Continue' }));
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await screen.findByRole('button', { name: 'Run tests' });
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open other' }));
  await screen.findByRole('heading', { name: 'other', level: 1 });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1));
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe('Run your first agentRunning');
  await user.click(screen.getByRole('button', { name: 'Overview' })); finish();
  await screen.findByText('The workspace is stopped. Start it to test or chat.', {}, { timeout: 2000 });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe('Run your first agentIts test passed');
});

test('a requested document switch does not admit the pending test while the bound sample has unsaved edits', async () => {
  const { ipc, finish } = native(true); const user = userEvent.setup(); render(<App />);
  await openLocalSetup(user);
  await user.click(await screen.findByRole('button', { name: 'Continue' }));
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await user.click(screen.getByRole('button', { name: 'File' }));
  await user.type(await screen.findByRole('textbox', { name: 'Agent file' }), ' Unsaved.');
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open other' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await screen.findByText('Connected', { selector: '.steps-head .pill' });
  await new Promise(resolve => setTimeout(resolve, 550));
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(0);
  expect(localStorage.getItem('bridge.firstTest.v1')).toBe('pending');
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')?.querySelector('.status-stale')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Stop the workspace' })).toBeTruthy();
  // A subsequently observed report ends the first-test step without turning unsaved edits into a pass.
  finish();
  await waitFor(() => expect(screen.queryByText('Setting up, step 4 of 4')).toBeNull(), { timeout: 2000 });
  expect(within(screen.getByRole('region', { name: 'Workspace setup' })).getByText('Run your first agent', { selector: 'li b' }).closest('li')?.querySelector('.status-stale')).toBeTruthy();
});

test.each([false, true])('Overview refreshes after the automatic test releases native admission without mounting Designer (lost replies: %s)', async lostReplies => {
  const { ipc, finish } = native(false, lostReplies); const user = await completeSetup();
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1));
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await screen.findByText('Updating when the test finishes.');
  finish();
  expect(await screen.findByText('The workspace is stopped. Start it to test or chat.', {}, { timeout: 2000 })).toBeTruthy();
  expect(screen.queryByText('Updating when the test finishes.')).toBeNull();
  expect(screen.getByRole('button', { name: 'Overview' }).getAttribute('aria-current')).toBe('page');
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1);
});

test.each(['Overview', 'Designer', 'cluster', 'Add', 'Places', 'workspace lost'])('first-test completion does not interrupt %s', async destination => {
  const { ipc, finish, loseWorkspace } = native(); const user = await completeSetup();
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1));
  if (destination === 'Overview' || destination === 'Designer') await user.click(screen.getByRole('button', { name: destination }));
  else if (destination === 'workspace lost') { loseWorkspace(); window.dispatchEvent(new Event('focus')); await screen.findByRole('button', { name: 'Continue' }); }
  else if (destination === 'Add' || destination === 'Places') {
    await user.click(screen.getByRole('button', { name: 'Clusters' }));
    if (destination === 'Add') await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  } else {
    await user.click(screen.getByRole('button', { name: 'Clusters' }));
    await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  }
  finish();
  await new Promise(resolve => setTimeout(resolve, 750));
  const expected = destination === 'Overview' || destination === 'Designer' ? destination : 'Clusters';
  expect(within(screen.getByRole('navigation')).getByRole('button', { name: expected }).getAttribute('aria-current')).toBe('page');
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1);
});

test('Add retires an automatic first test awaiting its initial observation without choosing another destination', async () => {
  const { ipc } = native();
  let resolve!: (value: unknown) => void; let reads = 0;
  mockIPC(command => {
    if (command === 'agent_run_status') { reads++; return new Promise(done => { resolve = done; }); }
    return ipc(command);
  });
  const user = await completeSetup();
  await waitFor(() => expect(reads).toBe(1));
  expect(localStorage.getItem('bridge.firstTest.v1')).toBe('pending');
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  expect(localStorage.getItem('bridge.firstTest.v1')).toBe('attempted');
  expect(document.querySelector('.host-chip .host-label')?.textContent).toBe('This computer');
  resolve(idle);
  await new Promise(done => setTimeout(done, 600));
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(0);
  expect(screen.getByRole('button', { name: 'Clusters' }).getAttribute('aria-current')).toBe('page');
  expect(screen.getByRole('button', { name: 'kubectl context' })).toBeTruthy();
});
