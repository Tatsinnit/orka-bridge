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
  let selected = false; let running = false; let finished = false;
  let unreadable = false; let unreadableFiles = false; let verdict = 'passed'; let saved = pair;
  const status = () => ({ selected: selected ? 'localVm' : null, localVm: selected ? 'ready' : 'idle', model: selected ? 'available' : 'locked' });
  const ipc = vi.fn((command: string, payload?: unknown) => {
    const args = payload as Record<string, unknown> | undefined;
    if (command === 'setup_status') return status();
    if (command === 'setup_requirements') return null;
    if (command === 'setup_select_local') { selected = true; if (lostReplies) throw Error('Lost selection reply'); return status(); }
    if (command === 'setup_change_local') { selected = false; return status(); }
    if (command === 'setup_example') return { agent: pair.agent, tests: pair.tests };
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'agent_select') return null;
    if (command === 'agent_open') { if (unreadableFiles) throw Error('Lost file read'); return args?.documentId === other ? { ...pair, documentId: other, name: 'other', agent: '---\nname: other\n---\nOther instructions.' } : { ...saved, documentId: sample, name: 'web-reader' }; }
    if (command === 'agent_save') { if (holdSave) return new Promise(() => {}); saved = { ...pair, agent: String(args?.content), source: { ...pair.source, agent: 'c'.repeat(64) } }; return { ...saved, documentId: sample, name: 'web-reader' }; }
    if (command === 'agent_run') { running = true; finished = false; if (lostReplies) throw Error('Lost run reply'); return { ...idle, state: 'running', canRun: false, busy: { documentId: sample, name: 'web-reader' } }; }
    if (command === 'agent_run_status') {
      if (args?.documentId === other) return { ...idle, documentId: other };
      if (unreadable) throw Error('Lost sample status');
      return finished ? { ...idle, source: pair.source, state: 'complete', report: { verdict, cases: [{ id: 'checksum', question: 'What checksum is on the page?', phase: 'Succeeded', answer: 'Correct checksum', completedTools: ['web_fetch'], assertions: [{ id: 'checksum', kind: 'contains', expected: 'Correct checksum', verdict }], verdict }] } } : running ? { ...idle, state: 'running', canRun: false, busy: { documentId: sample, name: 'web-reader' } } : idle;
    }
    if (command === 'send_history' || command === 'cluster_contexts') return [];
    if (command === 'overview_files') return { state: 'complete', files: [{ documentId: other, name: 'other', state: 'readable', source: null, summary: null, reason: null }], runActive: running && !finished };
    if (command === 'overview_agents') return { state: 'unavailable', installation: null, agents: [], observation: null, observedAt: null, runActive: false, reason: 'stopped' };
    throw Error('Unexpected command: ' + command);
  });
  mockIPC(ipc); return { ipc, finish: () => { finished = true; }, loseWorkspace: () => { selected = false; }, loseRunStatus: () => { unreadable = true; }, loseFileReads: () => { unreadableFiles = true; }, restoreFileReads: () => { unreadableFiles = false; }, failNext: () => { verdict = 'failed'; } };
}
beforeEach(() => localStorage.setItem('bridge.welcomeSeen.v1', '1'));
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

async function completeSetup() {
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Continue' }));
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }, { timeout: 2000 }));
  return user;
}

test('first setup shows a running test, opens Designer on its result and never auto-runs on another visit', async () => {
  const { ipc, finish } = native(); const user = await completeSetup();
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1));
  expect(screen.getByRole('button', { name: 'Clusters' }).getAttribute('aria-current')).toBe('page');
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe('Run your first agentRunning');
  expect(screen.getByText('Step 5 of 5')).toBeTruthy();
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.getAttribute('aria-current')).toBe('step');
  expect(screen.queryByRole('button', { name: 'See what it answered' })).toBeNull();
  finish();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Designer' }).getAttribute('aria-current')).toBe('page'));
  expect(await screen.findByText('1 of 1 passed')).toBeTruthy();
  await user.click(screen.getByText('checksum', { selector: 'code' }));
  expect(await screen.findByText('Correct checksum', { selector: 'p' })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'See what it answered' }));
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toEqual([['agent_run', { documentId: sample, bundledExample: true }]]);
});

test.each(['passed', 'failed', 'unreadable', 'edited'] as const)('Setup retains the latest sample evidence (%s), not the automatic result or another document', async state => {
  const { finish, loseRunStatus, failNext } = native(); const user = await completeSetup();
  await waitFor(() => expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe('Run your first agentRunning')); finish();
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
  const expected = { passed: 'Its test passed', failed: 'Its test failed', unreadable: 'Result unknown', edited: 'Needs running' }[state];
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe(`Run your first agent${expected}`);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open other' }));
  await screen.findByRole('heading', { name: 'other', level: 1 });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(screen.getByText('Run your first agent', { selector: 'li b' }).closest('li')!.textContent).toBe(`Run your first agent${expected}`);
  const card = within(screen.getByRole('region', { name: 'Local cluster setup' }));
  expect(card.queryByText('Its test failed', { selector: 'p' })?.textContent).toBe(state === 'failed' ? 'Its test failed' : undefined);
  expect(screen.queryByText('Setting up, step 5 of 5')).toBeNull();
  if (state === 'failed') {
    expect(card.getByText('Failed', { selector: '.pill' }).className).toContain('status-failed');
    expect(within(screen.getByRole('navigation')).getByText('Its test failed')).toBeTruthy();
    expect(within(screen.getByRole('contentinfo')).getByText('Its test failed')).toBeTruthy();
    expect(document.querySelector('.host-chip .live-dot')?.className).toContain('bad');
    await user.click(screen.getByRole('button', { name: 'Back to clusters' }));
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
  await screen.findByText('1 of 1 passed');
  failNext(); await user.click(screen.getByRole('button', { name: 'Run tests' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(screen.queryByText('Setting up, step 5 of 5')).toBeNull();
  expect(screen.getByRole('region', { name: 'Local cluster setup' }).textContent).toContain('Running test');
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
  await screen.findByText('The local cluster is stopped. Start it to test or chat.', {}, { timeout: 2000 });
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
  await screen.findByText('Connected', { selector: '.model-state .pill' });
  await new Promise(resolve => setTimeout(resolve, 550));
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(0);
  expect(localStorage.getItem('bridge.firstTest.v1')).toBe('pending');
  expect(screen.getByText('Step 5 of 5').previousElementSibling?.className).toContain('status-stale');
  expect(screen.getByRole('button', { name: 'Stop the local cluster' })).toBeTruthy();
  // A subsequently observed report ends the first-test step without turning unsaved edits into a pass.
  finish();
  await waitFor(() => expect(screen.queryByText('Step 5 of 5')).toBeNull(), { timeout: 2000 });
  expect(within(screen.getByRole('region', { name: 'Local cluster setup' })).getByText('Tests need running').previousElementSibling?.className).toContain('status-stale');
});

test.each([false, true])('Overview refreshes after the automatic test releases native admission without mounting Designer (lost replies: %s)', async lostReplies => {
  const { ipc, finish } = native(false, lostReplies); const user = await completeSetup();
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1));
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await screen.findByText('Updating when the test finishes.');
  finish();
  expect(await screen.findByText('The local cluster is stopped. Start it to test or chat.', {}, { timeout: 2000 })).toBeTruthy();
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
    await user.click(screen.getByRole('button', { name: 'Back to clusters' }));
    if (destination === 'Add') await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  } else {
    await user.click(screen.getByRole('button', { name: 'Back to clusters' }));
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
  await user.click(screen.getByRole('button', { name: 'Back to clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  expect(localStorage.getItem('bridge.firstTest.v1')).toBe('attempted');
  expect(document.querySelector('.host-chip .host-label')?.textContent).toBe('This computer');
  resolve(idle);
  await new Promise(done => setTimeout(done, 600));
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(0);
  expect(screen.getByRole('button', { name: 'Clusters' }).getAttribute('aria-current')).toBe('page');
  expect(screen.getByRole('combobox', { name: 'kubectl context' })).toBeTruthy();
});
