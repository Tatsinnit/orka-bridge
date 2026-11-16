import { openLocalSetup } from './test-navigation';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../App';
import { SetupProgress, SetupSteps } from './Steps';

const pair = { agent: '---\nname: web-reader\ndescription: Answers from the web page named in the request\nkmx:\n  tools:\n    - kind: builtin\n      name: web_fetch\n---\nFetch before answering.', tests: 'The checksum check.' };
const progress = { phase: 'materializing', bytes: 1200000000, totalBytes: 3000000000, elapsedMs: 20000, remainingMs: 30000 };
function native(initial = 'installing', numbers: { phase: string; bytes: number | null; totalBytes: number | null; elapsedMs: number; remainingMs: number | null } | null = progress) {
  vi.stubGlobal('isTauri', true);
  let localVm = initial; let selected: 'localVm' | null = 'localVm'; let unavailable = false;
  const status = () => ({ selected, localVm, model: localVm === 'ready' ? 'available' : 'locked', modelSupported: true, workspaceProgress: ['installing', 'starting'].includes(localVm) ? numbers : null, localVmError: localVm === 'failed' ? 'PRIVATE_NATIVE_DETAIL' : null });
  const ipc = vi.fn((command: string) => {
    if (command === 'setup_status') { if (unavailable) throw Error('unreadable'); return status(); }
    if (command === 'setup_cancel_local') { localVm = 'cancelled'; return status(); }
    if (command === 'setup_select_local') { selected = 'localVm'; localVm = 'installing'; return status(); }
    if (command === 'setup_change_local') { localVm = 'idle'; selected = null; return status(); }
    if (command === 'setup_example') return pair;
    if (command === 'agent_open') return { ...pair, documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader' };
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'agent_run_status') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'idle', canRun: true, report: null, acknowledged: [], pending: null, error: null };
    throw new Error('Unexpected command: ' + command);
  }); mockIPC(ipc);
  return { ipc, ready: () => { localVm = 'ready'; }, fail: () => { localVm = 'failed'; }, unreadable: () => { unavailable = true; }, readable: () => { unavailable = false; } };
}
beforeEach(() => localStorage.setItem('bridge.welcomeSeen.v1', '1'));
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

test.each([
  ['stale', undefined, 'Run it again', 'Tests need running', 'stale', undefined],
  ['stale', 1, 'Run it again', 'Tests need running', 'stale', undefined],
  ['unknown', undefined, 'Check again', 'Test result unknown', 'stale', undefined],
  ['unknown', 1, 'Check again', 'Test result unknown', 'stale', undefined],
  ['running', undefined, undefined, 'Running test', 'running', undefined],
  ['passed', 0, 'See what it answered', 'Local cluster ready', 'passed', undefined],
  ['failed', undefined, 'Check again', 'Its test failed', 'failed', undefined],
  ['failed', 0, 'See what it answered', 'Its test failed', 'failed', undefined],
  ['failed', 1, 'See what it answered', 'Its test failed', 'failed', 'Its test failed'],
  ['failed', 2, 'See what it answered', 'Its test failed', 'failed', 'Its test failed'],
] as const)('workspace card separates %s evidence (%s failures) from setup', (firstCheck, count, primary, badge, tone, summary) => {
  render(<SetupProgress status={{ selected: 'localVm', localVm: 'ready', model: 'available' }} modelState="connected"
    firstCheck={firstCheck} firstTestResult={count === undefined ? undefined : { failed: count }}
    onCheckFirstTest={() => {}} onOpenExample={() => {}} busy={false} stopping={false}
    onCancel={() => {}} onRestart={() => {}} onChange={() => {}} />);
  const region = screen.getByRole('region', { name: 'Local cluster setup' });
  expect(within(region).queryByText('Setting up, step 4 of 4')).toBeNull();
  expect(within(region).getByText(badge, { selector: '.status-description' })).toBeTruthy();
  expect(region.querySelector('.pill')?.className).toContain(`status-${tone}`);
  expect(region.querySelector('.pill')?.textContent).not.toContain(badge);
  const result = within(region).queryByRole('button', { name: 'See what it answered' });
  expect(!!result).toBe(count !== undefined);
  if (result && (firstCheck === 'stale' || firstCheck === 'unknown')) expect(result.className).toBe('button button-secondary');
  expect(within(region).queryByText('Its test failed', { selector: 'p' })?.textContent).toBe(summary);
  if (summary) expect(within(region).getByRole('status').compareDocumentPosition(within(region).getByText(summary, { selector: 'p' })) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  if (primary) {
    const button = within(region).getByRole('button', { name: primary });
    expect(button.className).toBe('button button-primary');
  } else expect(region.querySelector('.button-primary')).toBeNull();
});

test('four fixed steps contain one native file bar and expandable details', async () => {
  native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await screen.findByRole('heading', { name: 'Getting your workspace ready' });
  const steps = screen.getByRole('list', { name: 'Setup steps' });
  expect(within(steps).getAllByRole('listitem')).toHaveLength(4);
  expect(screen.getByText('Installing, 1.1 GiB of 2.8 GiB · 30 seconds left in this file step')).toBeTruthy();
  const bar = screen.getByRole('progressbar');
  expect(screen.getAllByRole('progressbar')).toHaveLength(1);
  expect(Number(bar.getAttribute('aria-valuenow'))).toBe(40);
  expect(bar.closest('li')).toBe(within(steps).getAllByRole('listitem')[1]);
  expect(screen.getByRole('button', { name: 'Connect the AI' }).matches(':disabled')).toBe(false);
  expect(screen.queryByText('Checking cluster files')).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Show details' }));
  expect(screen.getByText('Checking cluster files')).toBeTruthy();
  expect(screen.getByText('20 seconds installing files.')).toBeTruthy();
});

test.each(['installing', 'starting'])('tab, footer and list agree on workspace step during %s', async state => {
  native(state); render(<App />); await openLocalSetup();
  expect(within(screen.getByRole('navigation')).getByText('Setting up, step 2 of 4')).toBeTruthy();
  expect(within(screen.getByRole('contentinfo')).getByText('Getting your workspace ready, step 2 of 4')).toBeTruthy();
  expect(document.querySelector('.setup-progress [aria-current="step"] b')?.textContent).toBe('Get your workspace ready');
  expect(document.querySelector('.setup-progress .pill')).toBeNull();
  expect(screen.getByRole('button', { name: 'Connect the AI' }).hasAttribute('disabled')).toBe(false);
  expect(screen.getByText('Pick any time')).toBeTruthy();
});

test('one setup stack stays independent of the AI and introduction stack', async () => {
  native(); render(<App />); await openLocalSetup();
  const model = (await screen.findByRole('heading', { name: 'Connect the AI' })).closest('section')!;
  const steps = screen.getByRole('list', { name: 'Setup steps' });
  const progress = screen.getByRole('region', { name: 'Local cluster setup' });
  expect(progress.closest('.setup-column')).toBeTruthy();
  expect(model.closest('.setup-column')).toBeTruthy();
  expect(model.closest('.setup-column')).not.toBe(progress.closest('.setup-column'));
  expect(steps.closest('section')).toBe(progress);
  expect(document.querySelector('.workspace-meet')?.closest('.setup-column')).toBe(model.closest('.setup-column'));
});

test.each([12000, 95000])('startup stays indeterminate with observed elapsed time at %s ms', async elapsedMs => {
  native('starting', { phase: 'starting', bytes: null, totalBytes: null, elapsedMs, remainingMs: null }); render(<App />); await openLocalSetup();
  const bar = await screen.findByRole('progressbar', { name: 'Local cluster setup progress' });
  expect(bar.hasAttribute('aria-valuenow')).toBe(false);
  expect(screen.getByText(`${elapsedMs / 1000} seconds elapsed`)).toBeTruthy();
  expect(screen.queryByText(/estimate|seconds left/)).toBeNull();
});

test('unavailable progress never fabricates bytes or time left, and the bar stays indeterminate', async () => {
  native('installing', null); render(<App />); await openLocalSetup();
  await screen.findByRole('list', { name: 'Setup steps' });
  expect(screen.getByRole('progressbar').hasAttribute('aria-valuenow')).toBe(false);
  expect(screen.queryByText(/seconds left|minute left|of .*GiB/)).toBeNull();
});

test('readiness removes the bar without moving completed rows', async () => {
  const { ready } = native('starting', { phase: 'starting', bytes: null, totalBytes: null, elapsedMs: 31000, remainingMs: null }); render(<App />); await openLocalSetup();
  await screen.findByRole('progressbar');
  const list = screen.getByRole('list', { name: 'Setup steps' });
  const rows = within(list).getAllByRole('listitem');
  ready();
  await waitFor(() => expect(screen.queryByRole('progressbar')).toBeNull(), { timeout: 2000 });
  expect(within(list).getAllByRole('listitem')).toEqual(rows);
  expect(rows.slice(0, 2).map(row => row.className)).toEqual(['step-done', 'step-done']);
  expect(within(screen.getByRole('navigation')).getByText('Setting up, step 3 of 4')).toBeTruthy();
});

test('startup details never reuse the previous file phase timing', async () => {
  native('starting', { phase: 'materializing', bytes: 100, totalBytes: 100, elapsedMs: 54000, remainingMs: 1000 });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(screen.getByRole('button', { name: 'Show details' }));
  expect(screen.queryByText(/54 seconds|second left|in this file step/)).toBeNull();
  expect(screen.getByRole('progressbar').hasAttribute('aria-valuenow')).toBe(false);
});

test('the active Clusters tab replaces the local setup header without workspace effects', async () => {
  const { ipc } = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  expect(screen.queryByRole('button', { name: 'Back to clusters' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Choose where agents run' })).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await screen.findByRole('region', { name: 'This computer' });
  expect(ipc.mock.calls.some(([cmd]) => ['setup_cancel_local', 'setup_change_local'].includes(cmd))).toBe(false);
  await openLocalSetup(user);
  expect(screen.queryByRole('button', { name: 'Back to clusters' })).toBeNull();
});

test.each(['installing', 'ready'])('returning from Designer keeps the %s local view; active Clusters returns to Places', async phase => {
  const { ipc } = native(phase); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  const local = screen.getByRole('region', { name: 'Local cluster setup' });
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(screen.getByRole('region', { name: 'Local cluster setup' })).toBe(local);
  expect(screen.queryByRole('button', { name: 'Use a cluster instead' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Use a cluster you already have instead' })).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(await screen.findByRole('region', { name: 'This computer' })).toBeTruthy();
  expect(document.querySelector('.host-chip')?.textContent).toContain('This computer');
  expect(ipc.mock.calls.some(([cmd]) => ['setup_cancel_local', 'setup_change_local', 'setup_select_local', 'cluster_admit', 'cluster_contexts'].includes(cmd))).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  await screen.findByRole('heading', { name: 'Add a cluster' });
  expect(screen.getByRole('button', { name: 'Back to clusters' })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(await screen.findByRole('region', { name: 'This computer' })).toBeTruthy();
});

test('clicking the active Clusters tab on Places cannot arm a later polling focus jump', async () => {
  const sim = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await screen.findByRole('region', { name: 'This computer' });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  const add = screen.getByRole('button', { name: 'Add a cluster' });
  add.focus();
  sim.ready();
  await waitFor(() => expect(within(screen.getByRole('region', { name: 'This computer' })).getByRole('button', { name: 'Connect the AI' })).toBeTruthy());
  expect(document.activeElement).toBe(add);
});

test('keyboard Cancel then start again calls native retry and keeps the Meet panel', async () => {
  const { ipc } = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  const cancel = await screen.findByRole('button', { name: 'Cancel' }); cancel.focus();
  await user.keyboard('{Enter}');
  const retry = await screen.findByRole('button', { name: 'Start again' });
  expect(screen.getByText('Setup is paused. Choose Start again to resume.')).toBeTruthy();
  retry.focus(); await user.keyboard(' ');
  await screen.findByRole('button', { name: 'Cancel' });
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_cancel_local')).toHaveLength(1);
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_select_local')).toHaveLength(1);
});

test('Meet opens the owned example for editing without Run commands', async () => {
  const { ipc } = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  expect(await screen.findByRole('heading', { name: 'web-reader' })).toBeTruthy();
  expect(screen.getByText('Answers from the web page named in the request')).toBeTruthy();
  expect(within(screen.getByRole('navigation')).getByText('No agent open')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Read its instructions' }));
  const main = screen.getByRole('main', { name: 'web-reader' });
  expect(within(main).getByText('Fetch before answering.')).toBeTruthy();
  const run = within(main).getByRole('button', { name: 'Run tests' });
  expect(run.matches(':disabled')).toBe(true);
  expect(run.title).toBe('Wait for the local cluster and AI to be ready');
  await user.click(within(main).getByRole('button', { name: 'File' }));
  const editor = screen.getByRole('textbox', { name: 'Agent file' }) as HTMLTextAreaElement;
  expect(editor.readOnly).toBe(false); editor.focus(); await user.keyboard('{Control>}s{/Control}');
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_open')).toHaveLength(1);
  expect(ipc.mock.calls.some(([c]) => ['agent_save', 'agent_run', 'agent_run_status', 'workspace_catalog'].includes(c))).toBe(false);
  expect(within(screen.getByRole('navigation')).getByText((_, node) => node?.tagName === 'SMALL' && node.textContent === 'web-reader, ready to edit')).toBeTruthy();
});

test('unreadable setup status keeps owned editing but never admits Run or discovery', async () => {
  const { ipc, unreadable } = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  unreadable();
  await waitFor(() => expect(within(screen.getByRole('contentinfo')).getAllByText('Local cluster unavailable').length).toBeGreaterThan(0));
  await user.click(screen.getByRole('button', { name: 'File' }));
  expect((screen.getByRole('textbox', { name: 'Agent file' }) as HTMLTextAreaElement).readOnly).toBe(false);
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_open')).toHaveLength(1);
  expect(ipc.mock.calls.some(([c]) => ['agent_save', 'agent_run_status', 'workspace_catalog'].includes(c))).toBe(false);
});

test('setup readiness does not reopen the already editable document', async () => {
  const { ipc, ready } = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await screen.findByRole('textbox', { name: 'Instructions' });
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_open')).toHaveLength(1);
  ready();
  await within(screen.getByRole('contentinfo')).findByText('Connect the AI, step 3 of 4', {}, { timeout: 2000 });
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_open')).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: 'File' }));
  expect((screen.getByRole('textbox', { name: 'Agent file' }) as HTMLTextAreaElement).readOnly).toBe(false);
});

test('an established Designer stays editable when a running workspace later fails', async () => {
  const { ipc, fail } = native('ready'); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await screen.findByRole('heading', { name: 'web-reader', level: 1 });
  await user.click(screen.getByRole('button', { name: 'File' }));
  const editor = screen.getByRole('textbox', { name: 'Agent file' }); fail();
  await within(screen.getByRole('contentinfo')).findByText("Bridge couldn't determine why the local cluster failed.", {}, { timeout: 2000 });
  expect(screen.getByRole('textbox', { name: 'Agent file' })).toBe(editor);
  expect((editor as HTMLTextAreaElement).readOnly).toBe(false);
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_open')).toHaveLength(1);
});

test('failure is one plain red step, native details stay behind disclosure', async () => {
  native('failed'); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain("Bridge couldn't determine why the local cluster failed.");
  expect(alert.textContent).toContain('Your local cluster and agent files are kept.');
  expect(alert.closest('li')?.classList.contains('step-failed')).toBe(true);
  expect(screen.queryByText('PRIVATE_NATIVE_DETAIL')).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Show details' }));
  expect(screen.getByText('PRIVATE_NATIVE_DETAIL')).toBeTruthy();
});

test('Designer opens owned editing while the first native setup observation is pending', async () => {
  const { ipc } = native(); let observe!: (value: unknown) => void;
  mockIPC(command => command === 'setup_status' ? new Promise(resolve => { observe = resolve; }) : ipc(command));
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  await screen.findByRole('heading', { name: 'web-reader', level: 1 });
  expect(ipc.mock.calls.filter(([command]) => command === 'agent_open')).toHaveLength(1);
  observe({ selected: 'localVm', localVm: 'installing', model: 'locked' });
  await within(screen.getByRole('contentinfo')).findByText('Getting your workspace ready, step 2 of 4');
  expect(screen.getByRole('button', { name: 'Run tests' }).matches(':disabled')).toBe(true);
});

test('opening Designer while Continue awaits native admission opens editing but not Run', async () => {
  const { ipc } = native(); let admit!: (value: unknown) => void; let started = false;
  mockIPC(command => command === 'setup_status' ? { selected: started ? 'localVm' : null, localVm: started ? 'installing' : 'idle', model: 'locked' }
    : command === 'setup_select_local' ? new Promise(resolve => { admit = resolve; }) : ipc(command));
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Continue' }));
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  await screen.findByRole('heading', { name: 'web-reader', level: 1 });
  expect(ipc.mock.calls.filter(([command]) => command === 'agent_open')).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Run tests' }).matches(':disabled')).toBe(true);
  started = true; admit({ selected: 'localVm', localVm: 'installing', model: 'locked' });
  await within(screen.getByRole('contentinfo')).findByText('Getting your workspace ready, step 2 of 4');
});

test('a failed example read is not presented as an endless load', async () => {
  const { ipc } = native(); mockIPC(command => command === 'setup_example' ? Promise.reject('unavailable') : ipc(command)); render(<App />); await openLocalSetup();
  expect(await screen.findByText('The example could not be opened. Try opening Designer.')).toBeTruthy();
  expect(screen.queryByText('Opening the example\'s introduction…')).toBeNull();
});

test.each(['running', 'passed', 'failed', 'unknown'] as const)('the last setup step reflects actual %s run evidence', async result => {
  const { ipc } = native('ready');
  const snapshot = { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: result === 'running' ? 'running' : 'complete', canRun: result !== 'running', report: result === 'running' ? null : { verdict: result, cases: [{ id: 'checksum', question: 'What did the page say?', phase: 'Succeeded', answer: 'checksum', completedTools: ['web_fetch'], verdict: result, assertions: [{ id: 'value', kind: 'contains', expected: 'checksum', verdict: result }] }] }, acknowledged: [], pending: null, error: null };
  mockIPC(command => command === 'agent_run_status' ? snapshot : ipc(command));
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  // A test result is shown only for a ready workspace whose AI is connected.
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await screen.findByText('Connected', { selector: '.model-state .pill' });
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await screen.findByRole('heading', { name: 'web-reader', level: 1 });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await waitFor(() => expect(screen.getByText('Run your first test', { selector: 'li b' }).closest('li')!.textContent).toContain({ running: 'Running', passed: 'Its test passed', failed: 'Its test failed', unknown: 'Result unknown' }[result]));
  if (result === 'running') expect(screen.getByText('Run your first test', { selector: 'li b' }).closest('li')!.getAttribute('aria-current')).toBeNull();
});

test('stopping and starting the workspace preserves an owned draft after a failed save', async () => {
  const { ipc } = native('ready'); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await screen.findByRole('heading', { name: 'web-reader', level: 1 });
  await user.click(screen.getByRole('button', { name: 'File' }));
  const editor = screen.getByRole('textbox', { name: 'Agent file' }) as HTMLTextAreaElement;
  await user.type(editor, 'Retained draft');
  await screen.findByText('Your changes could not be saved. Your draft is still here.');
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await user.click(await screen.findByRole('button', { name: 'Stop the local cluster' }));
  await user.click(screen.getByRole('button', { name: 'Stop workspace' }));
  await screen.findByRole('heading', { name: 'This computer', level: 1 });
  expect(screen.queryByRole('button', { name: 'Continue' })).toBeNull();
  await user.click(await screen.findByRole('button', { name: 'Start' }));
  await screen.findByRole('button', { name: 'Cancel' });
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect(screen.getByRole('textbox', { name: 'Agent file' })).toBe(editor);
  expect(editor.value).toContain('Retained draft');
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_open')).toHaveLength(1);
});

test('the AI step resets after a connected workspace leaves ready', async () => {
  const { fail } = native('ready'); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await screen.findByText('Connected', { selector: '.model-state .pill' }); fail();
  await within(screen.getByRole('contentinfo')).findByText("Bridge couldn't determine why the local cluster failed.", {}, { timeout: 2000 });
  expect(screen.queryByText('Connected', { selector: 'li span' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Remove sign-in' })).toBeNull();
});

test('a delayed Cancel shows pending stop near the top instead of old byte progress', async () => {
  const { ipc } = native(); let done!: (value: unknown) => void;
  mockIPC(command => command === 'setup_cancel_local' ? new Promise(resolve => { done = resolve; }) : ipc(command));
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Cancel' }));
  expect(screen.getByText('Stopping the local cluster…')).toBeTruthy();
  expect(screen.queryByRole('progressbar')).toBeNull();
  done({ selected: 'localVm', localVm: 'cancelled', model: 'locked' });
  await screen.findByRole('button', { name: 'Start again' });
});

test('a delayed Stop cannot strand subsequent Start actions or duplicate native mutations', async () => {
  const { ipc } = native('ready'); let connect!: (value: unknown) => void; let stop!: (value: unknown) => void;
  let status = { selected: 'localVm', localVm: 'ready', model: 'available' };
  const commands = vi.fn(command => command === 'setup_status' ? status
    : command === 'setup_connect_model' ? new Promise(resolve => { connect = resolve; })
    : command === 'setup_cancel_local' ? new Promise(resolve => { stop = resolve; })
    : command === 'setup_select_local' ? (status = { selected: 'localVm', localVm: 'installing', model: 'locked' })
    : ipc(command));
  mockIPC(commands);
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  expect(screen.getByRole('button', { name: 'Stop the local cluster' })).toBeTruthy();
  connect({ ready: true }); await screen.findByText('Connected', { selector: '.model-state .pill' });
  await user.click(await screen.findByRole('button', { name: 'Stop the local cluster' }));
  await user.dblClick(screen.getByRole('button', { name: 'Stop workspace' }));
  expect(within(screen.getByRole('navigation')).getByText('This computer, stopping')).toBeTruthy();
  expect(within(screen.getByRole('contentinfo')).getByText('Workspace stopping')).toBeTruthy();
  expect(within(screen.getByRole('region', { name: 'Workspace' })).getByText('Stopping', { selector: '.pill' }).className).toContain('status-running');
  const pendingStart = screen.getByRole('button', { name: 'Start' });
  expect(pendingStart.matches(':disabled')).toBe(true);
  await user.click(pendingStart);
  expect(commands.mock.calls.filter(([cmd]) => cmd === 'setup_cancel_local')).toHaveLength(1);
  expect(commands.mock.calls.some(([cmd]) => cmd === 'setup_select_local')).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  const row = screen.getByRole('region', { name: 'This computer' });
  expect(within(row).getByText('Workspace stopping').closest('.pill')).toBeNull();
  expect(row.querySelector('.pill.status-running')).toBeTruthy();
  await openLocalSetup(user);
  status = { selected: 'localVm', localVm: 'cancelled', model: 'locked' }; stop(status);
  await screen.findByRole('heading', { name: 'This computer', level: 1 });
  expect(screen.queryByRole('button', { name: 'Continue' })).toBeNull();
  const next = await screen.findByRole('button', { name: 'Start' });
  expect(next.matches(':disabled')).toBe(false);
  await user.click(next);
  await screen.findByRole('button', { name: 'Cancel' });
  expect(commands.mock.calls.filter(([cmd]) => cmd === 'setup_select_local')).toHaveLength(1);
  expect(commands.mock.calls.some(([cmd]) => cmd === 'setup_change_local')).toBe(false);
});

test('Cancel and Start again restore keyboard focus after replacing the action', async () => {
  native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Cancel' }));
  await screen.findByRole('button', { name: 'Start again' });
  expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Getting your workspace ready', level: 1 }));
  await user.click(screen.getByRole('button', { name: 'Start again' }));
  await screen.findByRole('button', { name: 'Cancel' });
  expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Getting your workspace ready', level: 1 }));
});

test('a failed Stop retains the selected local cluster and never offers another setup admission', async () => {
  const { ipc } = native('ready');
  mockIPC(command => command === 'setup_cancel_local' ? Promise.reject('lost reply') : ipc(command));
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await user.click(await screen.findByRole('button', { name: 'Stop the local cluster' }));
  await user.click(screen.getByRole('button', { name: 'Stop workspace' }));
  expect(await screen.findByRole('alert')).toBeTruthy();
  expect(screen.getByRole('heading', { name: 'This computer', level: 1 })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Start' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Continue' })).toBeNull();
  expect(ipc.mock.calls.some(([c]) => c === 'setup_select_local')).toBe(false);
});

test('Stop is an explicit local-page action and leaves that page ready to restart', async () => {
  const { ipc } = native('ready'); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  expect(ipc.mock.calls.some(([c]) => c === 'setup_change_local')).toBe(false);
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await user.click(await screen.findByRole('button', { name: 'Stop the local cluster' }));
  expect(screen.getByRole('group', { name: 'Stop the workspace?' })).toBeTruthy();
  expect(ipc.mock.calls.some(([c]) => c === 'setup_cancel_local')).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Keep running' }));
  expect(screen.queryByRole('group', { name: 'Stop the workspace?' })).toBeNull();
  expect(ipc.mock.calls.some(([c]) => c === 'setup_cancel_local')).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Stop the local cluster' }));
  await user.click(screen.getByRole('button', { name: 'Stop workspace' }));
  expect(await screen.findByRole('heading', { name: 'This computer', level: 1 })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Start' }).matches(':disabled')).toBe(false);
  expect(screen.queryByRole('button', { name: 'Continue' })).toBeNull();
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_cancel_local')).toHaveLength(1);
  expect(ipc.mock.calls.some(([c]) => c === 'setup_change_local')).toBe(false);
});

test('an unreadable status makes the card unavailable instead of keeping the last running snapshot, and recovery keeps the queued choice', async () => {
  const { ipc, unreadable, readable } = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  expect(screen.getByRole('progressbar')).toBeTruthy();
  const card = () => within(screen.getByRole('region', { name: 'Local cluster setup' }));
  expect(card().getByText('Get your workspace ready').closest('li')?.getAttribute('aria-current')).toBe('step');
  unreadable();
  await waitFor(() => expect(within(screen.getByRole('contentinfo')).getAllByText('Local cluster unavailable').length).toBeGreaterThan(0), { timeout: 3000 });
  expect(card().getByRole('status').textContent).toBe('Local cluster status is unavailable.');
  expect(card().queryByText('Setting up, step 2 of 4')).toBeNull();
  expect(screen.queryByRole('progressbar')).toBeNull();
  expect(screen.queryByText(/Installing, 1.1 GiB/)).toBeNull();
  expect(card().getByText('Local cluster status is unavailable.')).toBeTruthy();
  const steps = screen.getByRole('list', { name: 'Setup steps' });
  // The list is a last-known snapshot, not a live observation: nothing runs and no row is current.
  expect(steps.querySelector('.step-current, [aria-current]')).toBeNull();
  expect(within(steps).getAllByText(/^Last known: /)).toHaveLength(4);
  expect(screen.getByText('Queued', { selector: '.pill' })).toBeTruthy();
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_connect_model')).toHaveLength(0);
  readable();
  await waitFor(() => expect(screen.getByRole('progressbar')).toBeTruthy(), { timeout: 3000 });
  expect(card().getByText('Get your workspace ready').closest('li')?.getAttribute('aria-current')).toBe('step');
  expect(within(steps).queryByText(/^Last known: /)).toBeNull();
  expect(screen.getByText('Queued', { selector: '.pill' })).toBeTruthy();
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_connect_model')).toHaveLength(0);
});

test.each([
  ['a workspace that is installing again', { selected: 'localVm', localVm: 'installing', model: 'locked' }, 'idle'],
  ['a disconnected workspace', { selected: 'localVm', localVm: 'ready', model: 'available' }, 'idle'],
  ['a failed AI connection', { selected: 'localVm', localVm: 'ready', model: 'available' }, 'failed'],
] as const)('a kept passed result is not shown for %s', (_name, status, modelState) => {
  render(<SetupSteps status={status} busy={false} stopping={false} modelState={modelState} firstCheck="passed" onRestart={() => {}} />);
  expect(screen.getByText('Run your first test', { selector: 'li b' }).closest('li')!.textContent).toBe('Run your first testWaiting');
});

test('stopping takes precedence over the last ready workspace row', () => {
  render(<SetupSteps status={{ selected: 'localVm', localVm: 'ready', model: 'available' }} busy stopping modelState="connected" onRestart={() => {}} />);
  expect(screen.getByText('Get your workspace ready', { selector: 'li b' }).closest('li')!.textContent).toBe('Get your workspace readyStopping');
});

test('a passed result is shown only once the workspace is ready and the AI is connected', () => {
  render(<SetupSteps status={{ selected: 'localVm', localVm: 'ready', model: 'available' }} busy={false} stopping={false} modelState="connected" firstCheck="passed" onRestart={() => {}} />);
  expect(screen.getByText('Run your first test', { selector: 'li b' }).closest('li')!.textContent).toBe('Run your first testIts test passed');
});
