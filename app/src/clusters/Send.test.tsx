import { openLocalSetup } from '../setup/test-navigation';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Example from '../designer/Example';
import App from '../App';
import Send from './Send';

const target = { id: '00000000-0000-4000-8000-000000000002', context_id: '00000000-0000-4000-8000-000000000001', context_name: 'team-dev', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'ghcr.io/orka:0.9.4' };
const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'team-model', ready: true };
const place = { kind: 'cluster' as const, target, providers: [provider], providerId: provider.id };
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const pair = { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: '---\nname: web-reader\nmodel: literal-model\n---\nRead the page.\n', tests: 'Checks from disk', source };
const idle = { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'idle', canRun: false, modelConnected: false, report: null, acknowledged: [], pending: null, error: null };
const report = { verdict: 'failed', cases: [{ id: 'reads-the-page', question: 'What did the page say?', phase: 'Succeeded', answer: 'Not the expected text', completedTools: [], assertions: [{ id: 'has-text', kind: 'contains', expected: 'checksum', verdict: 'failed' }], verdict: 'failed' }] };
const running = { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', id: '00000000-0000-4000-8000-000000000004', targetId: target.id, providerId: provider.id, targetName: target.context_name, source, model: 'literal-model', agentName: 'bridge-run-frozen', state: 'running', stage: 'observe', acknowledged: [{ kind: 'Agent', name: 'bridge-run-frozen', uid: 'uid-agent' }], pending: null, report: null, error: null, storageError: false };
const allowed = { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', source, targetId: target.id, providerId: provider.id, model: 'literal-model', rows: [{ label: 'Provider', status: 'allowed', message: 'Ready; the file model is tested by checks.' }], canSend: true };
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });
function native(local: unknown = idle, initial: unknown = null, preflight: unknown = allowed) {
  vi.stubGlobal('isTauri', true); let send = initial;
  const ipc = vi.fn((command: string) => {
    if (command === 'agent_open') return pair;
    if (command === 'agent_run_status') return local;
    if (command === 'send_status') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', view: send, busy: null };
    if (command === 'send_history') return [];
    if (command === 'send_preflight') return preflight;
    if (command === 'send_start') { send = running; return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', view: send, busy: null }; }
    if (command === 'send_stop') { send = { ...running, state: 'unknown', stage: 'stopped' }; return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', view: send, busy: null }; }
    throw Error('Unsupported');
  });
  mockIPC(ipc); return ipc;
}
test.each(['blocked', 'cannotCheck'])('preflight %s explains the problem and prevents submission', async status => {
  const ipc = native(idle, null, { ...allowed, rows: [{ label: 'page-reader', status, message: status === 'blocked' ? 'Create page-reader on team-dev.' : 'Bridge cannot check page-reader.' }], canSend: false });
  render(<Send place={place} source={source} stale={false} visible remote />);
  expect(await screen.findByText(status === 'blocked' ? 'Create page-reader on team-dev.' : 'Bridge cannot check page-reader.')).toBeTruthy();
  expect((screen.getByRole('button', { name: 'Send and run tests' }) as HTMLButtonElement).disabled).toBe(true);
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_start')).toBe(false);
});
test('a persistent polite status announces the final Send verdict', async () => {
  vi.stubGlobal('isTauri', true); let view: unknown = running;
  mockIPC(command => command === 'send_status' ? { documentId: pair.documentId, view, busy: null } : command === 'send_history' ? [] : allowed);
  render(<Send place={place} source={source} stale={false} visible remote />);
  await screen.findByText('Waiting for answers and evaluating tests');
  const live = screen.getAllByRole('status').find(node => node.classList.contains('sr-only'));
  expect(live).toBeTruthy();
  view = { ...running, state: 'complete', stage: 'done', report };
  await waitFor(() => expect(live?.textContent).toBe('Test result failed. 0 of 1 passed.'));
  expect(live?.isConnected).toBe(true);
});
test('stale local results do not gate checking or sending a saved pair', async () => {
  native(); render(<Send place={place} source={source} stale visible remote />);
  await waitFor(() => expect((screen.getByRole('button', { name: 'Send and run tests' }) as HTMLButtonElement).disabled).toBe(false));
});
test('capture failure shows the native check explanation, not a stale-file claim', async () => {
  native(idle, null, { ...allowed, source: null, model: null, rows: [{ label: 'Saved files', status: 'cannotCheck', message: 'Your saved files could not be read.' }], canSend: false });
  render(<Send place={place} source={source} stale={false} visible remote />);
  expect(await screen.findByText('Your saved files could not be read.')).toBeTruthy();
  expect(screen.queryByText(/saved files changed/)).toBeNull();
});
test('preflight pending blocks Send and only matching source enables it', async () => {
  let finish!: (value: unknown) => void;
  vi.stubGlobal('isTauri', true);
  mockIPC(command => command === 'send_preflight' ? new Promise(resolve => { finish = resolve; }) : null);
  render(<Send place={place} source={source} stale={false} visible remote />);
  await screen.findByText('Checking this destination…');
  expect((screen.getByRole('button', { name: 'Send and run tests' }) as HTMLButtonElement).disabled).toBe(true);
  finish({ ...allowed, source: { ...source, tests: 'c'.repeat(64) } });
  await screen.findByText(/saved files changed/);
  expect((screen.getByRole('button', { name: 'Send and run tests' }) as HTMLButtonElement).disabled).toBe(true);
});
test('a fresh preflight file change makes the earlier pass stale before Reload', async () => {
  const passed = { verdict: 'passed', cases: [{ ...report.cases[0], answer: 'checksum', verdict: 'passed', assertions: [{ id: 'has-text', kind: 'contains', expected: 'checksum', verdict: 'passed' }] }] };
  native(idle, { ...running, state: 'complete', stage: 'done', report: passed }, { ...allowed, source: { ...source, tests: 'c'.repeat(64) } });
  const view = render(<Send place={place} source={source} stale={false} visible remote />);
  await screen.findByRole('button', { name: 'Reload saved files' });
  expect(screen.getByText('Not run since your edit')).toBeTruthy();
  expect(screen.getAllByText('Test file changed since this run.').some(node => !node.classList.contains('test-announcement'))).toBe(true);
  expect(screen.getAllByRole('status').find(node => node.classList.contains('test-announcement'))?.textContent).toBe('Test file changed since this run.');
  expect(screen.getByText('reads-the-page').closest('details')?.querySelector('.status-passed')).toBeNull();
  view.rerender(<Send place={place} source={source} stale={false} blocked="Wait for your saved files to reload" visible remote />);
  expect(screen.getByText('reads-the-page').closest('details')?.querySelector('.status-passed')).toBeNull();
});
test.each(['failed', 'editorMismatch'])('preflight %s cannot promote an earlier pass', async mode => {
  const passed = { verdict: 'passed', cases: [{ ...report.cases[0], answer: 'checksum', verdict: 'passed', assertions: [{ id: 'has-text', kind: 'contains', expected: 'checksum', verdict: 'passed' }] }] };
  native(idle, { ...running, state: 'complete', stage: 'done', report: passed });
  if (mode === 'failed') mockIPC(command => {
    if (command === 'send_preflight') throw Error('unavailable');
    if (command === 'send_history') return [];
    return { documentId: pair.documentId, view: { ...running, state: 'complete', stage: 'done', report: passed }, busy: null };
  });
  render(<Send place={place} source={mode === 'editorMismatch' ? { ...source, tests: 'd'.repeat(64) } : source} stale={false} visible remote />);
  await screen.findByRole('button', { name: mode === 'failed' ? 'Check again' : 'Reload saved files' });
  await screen.findByText('reads-the-page');
  expect(screen.getByText('reads-the-page').closest('details')?.querySelector('.status-passed')).toBeNull();
});
test.each(['retained', 'discarded', 'stopped'])('unknown Send with %s evidence announces its actual state', async mode => {
  native(idle, { ...running, state: 'unknown', stage: mode === 'stopped' ? 'stopped' : 'done', report: mode === 'retained' ? report : null, error: 'storage' });
  render(<Send place={place} source={source} stale={false} visible remote />);
  const expected = mode === 'stopped' ? 'Stopped watching.' : 'Test result unknown.';
  await waitFor(() => expect(screen.getAllByRole('status').find(node => node.classList.contains('test-announcement'))?.textContent).toBe(expected));
});
test('changed disk files can be explicitly reloaded before preflight is retried', async () => {
  vi.stubGlobal('isTauri', true); let opened = false;
  const changed = { ...pair, agent: pair.agent.replace('literal-model', 'file-model').replace('Read the page.', 'Changed on disk.'), source: { ...source, tests: 'c'.repeat(64) } };
  mockIPC(command => {
    if (command === 'agent_open') { const value = opened ? changed : pair; opened = true; return value; }
    if (command === 'agent_run_status') return idle;
    if (command === 'send_status') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', view: null, busy: null };
    if (command === 'send_preflight') return { ...allowed, source: changed.source, model: 'file-model' };
    throw Error('Unsupported');
  });
  const user = userEvent.setup(); render(<Example remote place={place} />);
  await user.click(await screen.findByRole('button', { name: 'Reload saved files' }));
  await waitFor(() => expect((screen.getByRole('button', { name: 'Run all tests' }) as HTMLButtonElement).disabled).toBe(false));
  expect((screen.getByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement).value).toBe('Changed on disk.\n');
  expect(screen.getByText((_, node) => node?.tagName === 'DD' && node.textContent === 'file-model through team-model · AI connection ready; the model is checked when tests run')).toBeTruthy();
});
test('a delayed reload never overwrites edits made while files are being read', async () => {
  vi.stubGlobal('isTauri', true); let opened = false; let finish!: (value: unknown) => void;
  const changed = { ...pair, agent: pair.agent.replace('Read the page.', 'Disk replacement.'), source: { ...source, tests: 'c'.repeat(64) } };
  mockIPC((command, args) => {
    if (command === 'agent_open') { if (opened) return new Promise(resolve => { finish = resolve; }); opened = true; return pair; }
    if (command === 'agent_run_status') return idle;
    if (command === 'send_status') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', view: null, busy: null };
    if (command === 'send_preflight') return { ...allowed, source: changed.source };
    if (command === 'agent_save') return { ...pair, agent: (args as Record<string, string>).content };
    throw Error('Unsupported');
  });
  const user = userEvent.setup(); render(<Example remote place={place} />);
  await user.click(await screen.findByRole('button', { name: 'Reload saved files' }));
  const instructions = screen.getByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement;
  await user.clear(instructions); await user.type(instructions, 'My draft');
  await act(async () => { finish(changed); });
  expect(instructions.value).toBe('My draft');
  expect(instructions.value).not.toContain('Disk replacement');
});
test('collision is unverified, not refused; details disclosure names its state', async () => {
  native(idle, { ...running, state: 'complete', stage: 'done', report },  { ...allowed, rows: [{ label: 'page-reader', status: 'unverified', message: 'Implementation unverified: a built-in has this name.' }] });
  const user = userEvent.setup(); render(<Send place={place} source={source} stale={false} visible remote />);
  expect(await screen.findByText(/Implementation unverified/)).toBeTruthy();
  expect((screen.getByRole('button', { name: 'Send and run tests' }) as HTMLButtonElement).disabled).toBe(false);
  const details = document.querySelector<HTMLDetailsElement>('.long-card > .card-details')!;
  await user.click(details.querySelector('summary')!);
  expect(details.open).toBe(true);
});
test.each([
  { state: 'running', stage: 'capture', error: null, label: 'Running' },
  { state: 'unknown', stage: 'capture', error: 'capture', label: 'Result unknown' },
  { state: 'unknown', stage: 'stopped', error: 'stopped', label: 'Stopped watching' },
])('a source not yet captured keeps its $label status', async ({ state, stage, error, label }) => {
  native(idle, { ...running, source: null, state, stage, error });
  const { container } = render(<Send place={place} source={source} stale={false} visible remote />);
  if (state === 'running') await screen.findByText('Reading your saved files');
  await screen.findByText(label, { exact: true });
  expect(container.querySelector('.pill.status-stale')).toBeNull();
});
test('pending destination checks are blue running, not waiting on the creator', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => command === 'send_preflight' ? new Promise(() => {}) : null);
  const { container } = render(<Send place={place} source={source} stale={false} visible remote />);
  await screen.findByText('Checking this destination…');
  expect(container.querySelector('.pill.status-running')).toBeTruthy();
  expect(container.querySelector('.pill.status-needs-you')).toBeNull();
});
test('a late preflight for an old Provider cannot enable the current selection', async () => {
  let finish!: (value: unknown) => void;
  vi.stubGlobal('isTauri', true);
  mockIPC((command, args) => command === 'send_preflight'
    ? (args as Record<string, unknown> | undefined)?.providerId === provider.id ? new Promise(resolve => { finish = resolve; }) : new Promise(() => {}) : null);
  const view = render(<Send place={place} source={source} stale={false} visible remote />);
  await waitFor(() => expect(finish).toBeDefined());
  view.rerender(<Send place={{ ...place, providers: [provider, { ...provider, id: '00000000-0000-4000-8000-000000000005' }], providerId: '00000000-0000-4000-8000-000000000005' }} source={source} stale={false} visible remote />);
  finish(allowed);
  await screen.findByText('Checking this destination…');
  expect((screen.getByRole('button', { name: 'Send and run tests' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByText('Ready; the file model is tested by checks.')).toBeNull();
});
test('malformed preflight never displays raw fields or enables Send', async () => {
  native(idle, null, { ...allowed, privateCredential: 'PRIVATE_DIAGNOSTIC' });
  render(<Send place={place} source={source} stale={false} visible remote />);
  expect(await screen.findByText(/Destination checks could not be read/)).toBeTruthy();
  expect(document.body.textContent).not.toContain('PRIVATE_DIAGNOSTIC');
  expect((screen.getByRole('button', { name: 'Send and run tests' }) as HTMLButtonElement).disabled).toBe(true);
});
test('remote_checks_without_vm_or_local_model_gate', async () => {
  const ipc = native(); const user = userEvent.setup(); render(<Example remote place={place} />);
  await user.click(await screen.findByRole('button', { name: 'Run all tests' }));
  expect(await screen.findByText('Waiting for answers and evaluating tests')).toBeTruthy();
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'send_start')).toHaveLength(1);
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'agent_run' || cmd === 'setup_select_local' || cmd === 'setup_connect_model')).toBe(false);
});
test('failing_check_inline_note and fix_it_first_zero_post', async () => {
  const ipc = native({ ...idle, state: 'complete', source, report }); const user = userEvent.setup();
  render(<Example remote place={place} />);
  await waitFor(() => expect(ipc.mock.calls.some(([cmd]) => cmd === 'agent_run_status')).toBe(true));
  await user.click(screen.getByRole('button', { name: 'Send to a cluster' }));
  expect(screen.getByText(/reads-the-page.*failed/)).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Fix it first' }));
  expect(screen.queryByRole('button', { name: 'Send anyway' })).toBeNull();
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_start')).toBe(false);
});
test('send_anyway_explicit and duplicate clicks submit only once', async () => {
  const ipc = native({ ...idle, state: 'complete', source, report }); const user = userEvent.setup();
  render(<Example remote place={place} />); await waitFor(() => expect(ipc.mock.calls.some(([cmd]) => cmd === 'agent_run_status')).toBe(true));
  await user.click(screen.getByRole('button', { name: 'Send to a cluster' }));
  await user.click(screen.getByRole('button', { name: 'Send anyway' }));
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_start')).toBe(false);
  await user.dblClick(screen.getByRole('button', { name: 'Send and run tests' }));
  await screen.findByText('Waiting for answers and evaluating tests');
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'send_start')).toHaveLength(1);
});
test('Send entry returns keyboard focus to an already open panel without submitting', async () => {
  const ipc = native(); const user = userEvent.setup(); render(<Example remote place={place} />);
  const entry = await screen.findByRole('button', { name: 'Send to a cluster' });
  await user.click(entry);
  const panel = screen.getByRole('region', { name: 'Send and cluster tests' });
  await waitFor(() => expect(document.activeElement).toBe(panel));
  await user.click(entry);
  await waitFor(() => expect(document.activeElement).toBe(panel));
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_start')).toBe(false);
});
test('Stop shows retained receipts and remount observes without replay', async () => {
  const ipc = native(idle, running); const user = userEvent.setup(); const view = render(<Example remote place={place} />);
  await user.click(await screen.findByRole('button', { name: 'Stop' }));
  expect(await screen.findByText(/cluster may still be running checks/)).toBeTruthy();
  await user.click(within(screen.getByRole('region', { name: 'Send and cluster tests' })).getByText('Details'));
  expect(screen.getByRole('button', { name: 'Copy Agent' })).toBeTruthy();
  expect(screen.getAllByText('brid…ozen').length).toBeGreaterThan(0);
  view.unmount(); render(<Example remote place={place} />);
  await screen.findByText('Stopped watching. The cluster may still be running checks.');
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_start')).toBe(false);
});
test('no_only_you_pre_live_or_internal_jargon and honest unknown evidence', async () => {
  native(idle, { ...running, state: 'complete', stage: 'done', report: { ...report, verdict: 'unknown', cases: [{ ...report.cases[0], verdict: 'unknown', answer: null }] } });
  render(<Example remote place={place} />);
  expect(await screen.findByText('Result unknown')).toBeTruthy();
  expect(screen.getByText('No answer was observed.')).toBeTruthy();
  const panel = screen.getByRole('region', { name: 'Send and cluster tests' });
  expect(panel.textContent).not.toMatch(/only you|pre.live|guest|native|runtime|acknowledged|inference/i);
  expect(panel.querySelector('a')).toBeNull();
});
test('an unknown submission names its uncertain resources, not a pass, and waits for explicit resend', async () => {
  const ipc = native(idle, { ...running, state: 'unknown', stage: 'submit', error: 'submit', pending: { kind: 'Task', name: 'bridge-task-uncertain' } });
  const user = userEvent.setup(); render(<Example remote place={place} />);
  expect(await screen.findByText('Result unknown')).toBeTruthy();
  await user.click(within(screen.getByRole('region', { name: 'Send and cluster tests' })).getByText('Details'));
  expect(screen.getByRole('button', { name: 'Copy Possibly created Task' })).toBeTruthy();
  expect(screen.getByText('brid…tain')).toBeTruthy();
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_start')).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Run all tests' }));
  await screen.findByText('Waiting for answers and evaluating tests');
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'send_start')).toHaveLength(1);
});
test.each(['stopped', 'observe', 'submit'])('uncertain %s never claims a failed verdict', async error => {
  native(idle, { ...running, state: 'unknown', stage: error === 'stopped' ? 'stopped' : 'observe', error });
  const { container } = render(<Send place={place} source={source} stale={false} visible remote />);
  expect(await screen.findByText(error === 'stopped' ? 'Stopped watching' : 'Result unknown', { exact: true })).toBeTruthy();
  expect(container.querySelector('.pill.status-failed')).toBeNull();
});
test('changing Provider or destination does not validate the new selection with an older pass', async () => {
  native(idle, { ...running, state: 'complete', stage: 'done', report: { ...report, verdict: 'passed' } });
  const view = render(<Example remote place={place} />);
  await screen.findByText('0 of 1 passed');
  view.rerender(<Example remote place={{ ...place, providerId: '00000000-0000-4000-8000-000000000005' }} />);
  expect(await screen.findByText(/not your current selection/)).toBeTruthy();
  expect(document.querySelector('.send-panel .pill.status-passed')).toBeNull();
  expect(document.querySelector('.send-panel .pill.status-stale')).toBeTruthy();
  view.rerender(<Example remote place={{ ...place, target: { ...target, id: '00000000-0000-4000-8000-000000000006', context_name: 'other-cluster' } }} />);
  expect(screen.getByText(/not your current selection/)).toBeTruthy();
});
test('running results show a factual stage instead of a disabled primary beside Stop', async () => {
  const ipc = native(idle, running);
  render(<Send place={place} source={source} stale={false} visible remote />);
  expect(await screen.findByText('Waiting for answers and evaluating tests')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Send and run tests' })).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: 'Stop' }));
  expect(await screen.findByText('Stopped watching. The cluster may still be running checks.')).toBeTruthy();
  expect(ipc.mock.calls.filter(([cmd]) => cmd === 'send_stop')).toHaveLength(1);
});
test('cluster results use frozen questions and plain reasons; stale rows cannot look passed', async () => {
  native(idle, { ...running, state: 'complete', stage: 'done', report });
  const view = render(<Send place={place} source={source} stale={false} visible remote />);
  expect(await screen.findByText('What did the page say?')).toBeTruthy();
  expect(screen.getByText('The answer did not include “checksum”.')).toBeTruthy();
  view.rerender(<Send place={place} source={{ ...source, tests: 'c'.repeat(64) }} stale={false} visible remote />);
  expect(screen.getAllByText('Test file changed since this run.')).toHaveLength(2);
  expect(document.querySelector('.test-results .status-passed, .test-results .status-failed')).toBeNull();
});
test('malformed send data is not rendered or retried', async () => {
  const ipc = native(idle, { ...running, privateCredential: 'PRIVATE_DIAGNOSTIC' }); render(<Example remote place={place} />);
  expect(await screen.findByText(/Send information could not be read/)).toBeTruthy();
  expect(document.body.textContent).not.toContain('PRIVATE_DIAGNOSTIC');
  expect(ipc.mock.calls.some(([cmd]) => cmd === 'send_start')).toBe(false);
});
test('a confirmed cluster opens the owned editor after failed local setup and keeps Provider choice', async () => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  const other = { ...provider, id: '00000000-0000-4000-8000-000000000005', name: 'second-model', ready: true };
  let starts = 0; let sentArgs: unknown;
  mockIPC((command, args) => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'failed', localVmError: 'boot', model: 'locked' };
    if (command === 'setup_requirements') return null;
    if (command === 'cluster_contexts') return [{ id: target.context_id, name: target.context_name }];
    if (command === 'cluster_discover') return [target];
    if (command === 'cluster_use') return { status: 'connected', providers: [provider, other], failure: null };
    if (command === 'agent_open') return pair;
    if (command === 'agent_run_status') return idle;
    if (command === 'send_status') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', view: null, busy: null };
    if (command === 'send_preflight') return { ...allowed, providerId: other.id };
    if (command === 'send_start') { starts++; sentArgs = args; return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', view: { ...running, providerId: other.id }, busy: null }; }
    throw Error('Unsupported');
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(await screen.findByRole('button', { name: 'Use a cluster you already have instead' }));
  await user.selectOptions(await screen.findByRole('combobox', { name: 'kubectl context' }), target.context_id);
  await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  await user.selectOptions(await screen.findByRole('combobox', { name: 'AI connection' }), other.id);
  await user.click(screen.getByRole('button', { name: 'Open your agent' }));
  expect(await screen.findByRole('heading', { name: 'web-reader', level: 1 })).toBeTruthy();
  expect(screen.queryByText('Read-only during setup')).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Run all tests' }));
  await waitFor(() => expect(starts).toBe(1));
  expect(sentArgs).toEqual({ documentId: '9b2b93c4-375c-4815-9717-39852a96e411', targetId: target.id, providerId: other.id, expectedSource: source });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Back to clusters' }));
  await openLocalSetup(user);
  await user.click(screen.getByRole('button', { name: 'Use this computer' }));
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect((await screen.findByRole('textbox', { name: 'Instructions' }) as HTMLTextAreaElement).readOnly).toBe(false);
});
