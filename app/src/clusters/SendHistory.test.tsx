import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Send, { SendHistory } from './Send';
import Example from '../designer/Example';
import { SAMPLE_DOCUMENT_ID } from '../designer/agent-api';
const idle = { documentId: SAMPLE_DOCUMENT_ID, view: null, busy: null };

const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const summary = {
  id: '00000000-0000-4000-8000-000000000001', source,
  targetDisplay: 'team-dev · agents',
  targetUids: { serviceNamespace: 'control', serviceUid: 'service-1', deploymentNamespace: 'control', deploymentUid: 'deployment-1', watchedNamespace: 'agents' },
  provider: 'team-model', plannedAgentName: 'bridge-run-before-restart',
  agentUid: null, verdict: 'unknown', time: 1794412800,
};
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
function native(history: unknown) {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn((command: string) => {
    if (command === 'send_status') return idle;
    if (command === 'send_history') return history;
    if (command === 'send_history_details') return legacy;
    throw Error('Unexpected operation');
  });
  mockIPC(ipc); return ipc;
}
const panel = () => <Send place={{ kind: 'thisComputer' }} source={source} stale={false} visible remote={false} />;
const legacy = { id: summary.id, documentId: null, model: null, capturedAt: null, report: null, interrupted: false, canCheck: false, canRemove: false, detailState: 'legacy', availability: null };
const saved = {
  ...legacy, documentId: SAMPLE_DOCUMENT_ID, model: 'test-model', capturedAt: 1794412801,
  canCheck: true, canRemove: true, detailState: 'saved',
  report: { verdict: 'passed', cases: [{ id: 'old-test', question: 'Captured question', phase: 'Succeeded', answer: '<img src=x onerror=alert(1)> Saved answer', completedTools: [], verdict: 'passed', assertions: [{ id: 'old-rule', kind: 'contains', expected: 'Saved answer', verdict: 'passed' }] }] },
};
function detailed(value: unknown = saved, failure?: string) {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn((command: string) => {
    if (command === 'send_history') return [summary];
    if (command === 'send_history_details') return value;
    if (command === 'send_history_check') {
      if (failure) throw Error(failure);
      return { ...saved, availability: 'missing' };
    }
    if (command === 'send_history_remove') return { ...saved, detailState: 'removed', report: null, canRemove: false, canCheck: false };
    throw Error('Unexpected operation');
  });
  mockIPC(ipc); render(<SendHistory revision="reopened" />); return ipc;
}
async function openSaved() {
  await userEvent.setup().click(await screen.findByText('Show saved details'));
}

test('restart shows saved pass and unknown without observing or adopting either Agent', async () => {
  const ipc = native([summary, { ...summary, id: '00000000-0000-4000-8000-000000000002', plannedAgentName: 'bridge-run-passed', agentUid: 'agent-uid', verdict: 'passed' }]);
  render(panel());
  const history = await screen.findByRole('region', { name: 'Past Sends' });
  expect(within(history).getByText('bridge-run-before-restart')).toBeTruthy();
  expect(within(history).getByText('bridge-run-passed')).toBeTruthy();
  expect(within(history).getByText('Saved tests: unknown')).toBeTruthy();
  expect(within(history).getByText('Saved tests: pass')).toBeTruthy();
  expect(within(history).getByText(/interrupted Send may still be running/)).toBeTruthy();
  expect(within(history).getAllByText('team-dev · agents')).toHaveLength(2);
  expect(ipc.mock.calls.every(([command]) => ['send_status', 'send_history'].includes(command))).toBe(true);
  expect(within(history).queryByRole('button', { name: /resume|adopt/i })).toBeNull();
  const user = userEvent.setup();
  await user.click(within(history).getAllByText('Show saved details')[0]);
  expect(within(history).getByText('Not confirmed')).toBeTruthy();
  expect(within(within(history).getAllByRole('listitem')[0]).getByText(source.agent)).toBeTruthy();
});

test.each([
  [{ ...summary, verdict: 'invented-pass' }],
  [{ ...summary, token: 'PRIVATE_DIAGNOSTIC' }],
  [{ ...summary, source: { agent: 'not-a-hash', tests: source.tests } }],
  [{ ...summary, time: Number.MAX_SAFE_INTEGER }],
])('malformed stored summary is unavailable, never a pass or native diagnostic', async history => {
  native(history); render(panel());
  expect(await screen.findByText(/Saved Send summaries could not be read/)).toBeTruthy();
  expect(screen.queryByText('Checks: pass')).toBeNull();
  expect(document.body.textContent).not.toContain('PRIVATE_DIAGNOSTIC');
});

test('history read failure is explicit and retry only rereads local summaries', async () => {
  vi.stubGlobal('isTauri', true); let broken = true;
  const ipc = vi.fn((command: string) => {
    if (command === 'send_status') return idle;
    if (command === 'send_history') { if (broken) throw Error('PRIVATE_DIAGNOSTIC'); return [summary]; }
    throw Error('Unexpected operation');
  });
  mockIPC(ipc); render(panel());
  await screen.findByText(/Saved Send summaries could not be read/);
  broken = false;
  await userEvent.setup().click(screen.getByRole('button', { name: 'Retry saved Sends' }));
  expect(await screen.findByText('bridge-run-before-restart')).toBeTruthy();
  expect(screen.queryByText(/Saved Send summaries could not be read/)).toBeNull();
  expect(ipc.mock.calls.every(([command]) => ['send_status', 'send_history'].includes(command))).toBe(true);
});

test('saved history remains readable without a selected cluster or permission to send during setup', async () => {
  const ipc = native([summary]);
  render(<Send place={{ kind: 'thisComputer' }} source={source} stale={false} visible={false} remote={false} blocked="Read-only during setup" />);
  expect(await screen.findByRole('region', { name: 'Past Sends' })).toBeTruthy();
  expect(screen.getByText('bridge-run-before-restart')).toBeTruthy();
  expect(ipc.mock.calls.every(([command]) => ['send_status', 'send_history'].includes(command))).toBe(true);
});

test('global history refreshes when another document’s Send releases admission', async () => {
  vi.stubGlobal('isTauri', true);
  const other = '00000000-0000-4000-8000-000000000003';
  let busy: { documentId: string; name: string } | null = { documentId: other, name: 'other-agent' };
  let history: unknown[] = [];
  mockIPC(command => {
    if (command === 'send_status') return { ...idle, busy };
    if (command === 'send_history') return history;
    throw Error('Unexpected operation');
  });
  render(panel());
  await screen.findByText('No saved Sends yet.');
  history = [summary]; busy = null;
  expect(await screen.findByText('bridge-run-before-restart')).toBeTruthy();
});

test.each(['pending', 'failed'])('saved Sends are readable while the document open is %s', async state => {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn((command: string) => {
    if (command === 'agent_open') {
      if (state === 'failed') throw Error('PRIVATE_SOURCE_FAILURE');
      return new Promise(() => {});
    }
    if (command === 'send_history') return [summary];
    throw Error('Unexpected operation');
  });
  mockIPC(ipc); render(<Example />);
  expect(await screen.findByText('bridge-run-before-restart')).toBeTruthy();
  if (state === 'failed') expect(screen.getByText('Your agent folder could not be opened. Open it again from Designer.')).toBeTruthy();
  expect(document.body.textContent).not.toContain('PRIVATE_SOURCE_FAILURE');
  expect(ipc.mock.calls.every(([command]) => ['agent_open', 'send_history'].includes(command))).toBe(true);
});

test('empty history says no saved Sends, not that no remote Agents exist', async () => {
  native([]); render(panel());
  expect(await screen.findByText('No saved Sends yet.')).toBeTruthy();
  expect(screen.queryByText(/no agents/i)).toBeNull();
});

test('a failed initial save explains that nothing was sent, rather than suggesting remote work exists', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => {
    if (command === 'send_history') return [];
    if (command === 'send_status') return { ...idle, view: {
      documentId: SAMPLE_DOCUMENT_ID, id: summary.id, targetId: summary.id, providerId: summary.id, targetName: 'team-dev', source,
      model: 'model', agentName: summary.plannedAgentName, state: 'unknown', stage: 'submit',
      acknowledged: [], pending: null, report: null, error: 'storage', storageError: true,
    } };
    throw Error('Unexpected operation');
  });
  render(panel());
  expect(await screen.findByText(/Nothing was sent because Bridge could not save a summary/)).toBeTruthy();
  expect(screen.queryByText(/Named resources may still exist/)).toBeNull();
});

test.each(['submit', 'observe'])('storage failure during %s keeps remote uncertainty instead of claiming nothing was sent', async error => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => {
    if (command === 'send_history') return [];
    if (command === 'send_status') return { ...idle, view: {
      documentId: SAMPLE_DOCUMENT_ID, id: summary.id, targetId: summary.id, providerId: summary.id, targetName: 'team-dev', source,
      model: 'model', agentName: summary.plannedAgentName, state: 'unknown', stage: error,
      acknowledged: [{ kind: 'Agent', name: summary.plannedAgentName, uid: 'known-agent' }], pending: null, report: null, error, storageError: true,
    } };
    throw Error('Unexpected operation');
  });
  render(panel());
  expect(await screen.findByText(/could not save this Send/)).toBeTruthy();
  expect(screen.queryByText(/Nothing was sent/)).toBeNull();
  expect(screen.getByText(error === 'submit' ? /Named resources may still exist/ : /cluster may still be running checks/)).toBeTruthy();
});

test('a storage write failure stays visible with an unknown live result', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => {
    if (command === 'send_history') return [summary];
    if (command === 'send_status') return { ...idle, view: {
      documentId: SAMPLE_DOCUMENT_ID, id: summary.id, targetId: summary.id, providerId: summary.id, targetName: 'team-dev', source,
      model: 'model', agentName: summary.plannedAgentName, state: 'unknown', stage: 'done',
      acknowledged: [], pending: null, report: null, error: null, storageError: true,
    } };
    throw Error('Unexpected operation');
  });
  render(panel());
  await waitFor(() => expect(screen.getByText(/could not save this Send/)).toBeTruthy());
  expect(screen.getByText('Earlier result unknown')).toBeTruthy();
  expect(document.querySelector('.send-panel .pill.status-stale')).toBeTruthy();
  expect(screen.queryByText('Checks: pass')).toBeNull();
});

test('reopened answers load locally on demand, with captured time and safe plain text', async () => {
  const ipc = detailed();
  await screen.findByText('bridge-run-before-restart');
  expect(ipc.mock.calls.map(([command]) => command)).toEqual(['send_history']);
  await openSaved();
  expect(await screen.findByText('Saved on this computer')).toBeTruthy();
  expect(screen.getByText('Captured question')).toBeTruthy();
  expect(screen.getByText('<img src=x onerror=alert(1)> Saved answer')).toBeTruthy();
  expect(document.querySelector('.send-history img')).toBeNull();
  expect(screen.getAllByRole('time').some(t => t.getAttribute('datetime') === '2026-11-11T16:00:01.000Z')).toBe(true);
  expect(ipc.mock.calls.map(([command]) => command)).toEqual(['send_history', 'send_history_details']);
});

test.each(['passed', 'failed'])('a closed row follows its completed %s summary instead of cached running detail', async verdict => {
  vi.stubGlobal('isTauri', true);
  let complete = false;
  const ipc = vi.fn((command: string) => {
    if (command === 'send_history') return [{ ...summary, verdict: complete ? verdict : 'running' }];
    if (command === 'send_history_details') return { ...saved, report: null, interrupted: true, canCheck: false, canRemove: false };
    throw Error('Unexpected operation');
  });
  mockIPC(ipc);
  const view = render(<SendHistory revision="running" />);
  await openSaved();
  await screen.findByText('Last seen running; current result unknown');
  await userEvent.setup().click(screen.getByText('Show saved details'));
  await waitFor(() => expect(screen.queryByText('Last seen running; current result unknown')).toBeNull());
  complete = true;
  view.rerender(<SendHistory revision="complete" />);
  await screen.findByText(`Saved tests: ${verdict === 'passed' ? 'pass' : 'fail'}`);
  expect(ipc.mock.calls.filter(([command]) => command === 'send_history_details')).toHaveLength(1);
});

test('interrupted evidence is unknown without a live spinner and requires an explicit cluster check', async () => {
  const ipc = detailed({ ...saved, interrupted: true, canRemove: false });
  await openSaved();
  expect(await screen.findByText('Last seen running; current result unknown')).toBeTruthy();
  expect(document.querySelector('.send-history .status-running')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Remove saved details' })).toBeNull();
  expect(ipc.mock.calls.some(([command]) => command === 'send_history_check')).toBe(false);
  await userEvent.setup().click(screen.getByRole('button', { name: 'Check cluster status' }));
  expect(await screen.findByText('No longer available from the cluster')).toBeTruthy();
  expect(screen.getByText('<img src=x onerror=alert(1)> Saved answer')).toBeTruthy();
  expect(ipc.mock.calls.every(([command]) => ['send_history', 'send_history_details', 'send_history_check'].includes(command))).toBe(true);
});

test('remove saved details removes the local answer only and does not check or delete cluster work', async () => {
  const ipc = detailed(); await openSaved();
  await screen.findByText('Saved on this computer');
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Remove saved details' }));
  expect(ipc.mock.calls.some(([command]) => command === 'send_history_remove')).toBe(false);
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Keep saved details' }));
  await user.click(screen.getByRole('button', { name: 'Keep saved details' }));
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Remove saved details' }));
  await user.click(screen.getByRole('button', { name: 'Remove saved details' }));
  await user.click(screen.getByRole('button', { name: 'Remove local copy' }));
  expect(await screen.findByText('Saved details removed from this computer.')).toBeTruthy();
  expect(screen.queryByText('<img src=x onerror=alert(1)> Saved answer')).toBeNull();
  expect(ipc.mock.calls.map(([command]) => command)).toEqual(['send_history', 'send_history_details', 'send_history_remove']);
  expect(screen.getByText('bridge-run-before-restart')).toBeTruthy();
});

test('removing answers after a status check keeps the newly saved unknown verdict', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => {
    if (command === 'send_history') return [{ ...summary, verdict: 'passed' }];
    if (command === 'send_history_details') return saved;
    if (command === 'send_history_check') return { ...saved, availability: 'missing', report: { ...saved.report, verdict: 'unknown' } };
    if (command === 'send_history_remove') return { ...saved, detailState: 'removed', report: null, canRemove: false };
    throw Error('Unexpected operation');
  });
  render(<SendHistory revision="same" />); await openSaved();
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Check cluster status' }));
  await screen.findByText('Saved tests: unknown');
  await user.click(screen.getByRole('button', { name: 'Remove saved details' }));
  await user.click(screen.getByRole('button', { name: 'Remove local copy' }));
  await screen.findByText('Saved details removed from this computer.');
  expect(screen.getByText('Saved tests: unknown')).toBeTruthy();
  expect(screen.queryByText('Saved tests: pass')).toBeNull();
});

test('legacy summaries do not invent answers or offer recovery without captured identities', async () => {
  detailed(legacy); await openSaved();
  expect(await screen.findByText('This earlier Send has a summary only.')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Check cluster status' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Remove saved details' })).toBeNull();
  expect(screen.queryByText('Saved on this computer')).toBeNull();
});

test('omitted answer details are explicitly incomplete, not a complete saved pass', async () => {
  detailed({ ...saved, detailState: 'omitted', report: null }); await openSaved();
  expect(await screen.findByText('Some details could not be saved.')).toBeTruthy();
  expect(screen.queryByText('Saved on this computer')).toBeNull();
});

test.each([
  { ...saved, token: 'PRIVATE_DIAGNOSTIC' },
  { ...saved, id: '00000000-0000-4000-8000-000000000099' },
  { ...saved, report: { ...saved.report, cases: [{ ...saved.report.cases[0], answer: 'x'.repeat(1048577) }] } },
])('malformed detail payload is unavailable without displaying native or stored diagnostics', async value => {
  detailed(value); await openSaved();
  expect(await screen.findByText(/Saved details could not be read/)).toBeTruthy();
  expect(document.body.textContent).not.toContain('PRIVATE_DIAGNOSTIC');
  expect(screen.queryByText('Saved on this computer')).toBeNull();
});

test('closing and reopening details during a status check keeps its result and releases controls', async () => {
  vi.stubGlobal('isTauri', true);
  let finish: (value: unknown) => void = () => {};
  mockIPC(command => {
    if (command === 'send_history') return [summary];
    if (command === 'send_history_details') return saved;
    if (command === 'send_history_check') return new Promise(resolve => { finish = resolve; });
    throw Error('Unexpected operation');
  });
  render(<SendHistory revision="same" />); await openSaved();
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Check cluster status' }));
  await user.click(screen.getByText('Show saved details'));
  await user.click(screen.getByText('Show saved details'));
  finish({ ...saved, availability: 'missing' });
  expect(await screen.findByText('No longer available from the cluster')).toBeTruthy();
  expect((screen.getByRole('button', { name: 'Check cluster status' }) as HTMLButtonElement).disabled).toBe(false);
});

test('failed explicit status check keeps saved answers and hides diagnostics', async () => {
  detailed(saved, 'PRIVATE_DIAGNOSTIC'); await openSaved();
  await userEvent.setup().click(await screen.findByRole('button', { name: 'Check cluster status' }));
  expect(await screen.findByText(/Cluster status could not be checked/)).toBeTruthy();
  expect(screen.getByText('<img src=x onerror=alert(1)> Saved answer')).toBeTruthy();
  expect(document.body.textContent).not.toContain('PRIVATE_DIAGNOSTIC');
});
