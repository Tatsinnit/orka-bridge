import { StrictMode } from 'react';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { armFirstTest, retireFirstTest, useFirstTest } from './first-test';

const sample = '9b2b93c4-375c-4815-9717-39852a96e411';
const pair = { agent: '---\nname: web-reader\n---\nRead a page.\n', tests: 'A checksum test.\n' };
const idle = { documentId: sample, busy: null, state: 'idle', canRun: true, modelConnected: true, report: null, acknowledged: [], pending: null, error: null };
function native({ changed = false, crlf = false, unreadable = false, rejected = false } = {}) {
  vi.stubGlobal('isTauri', true);
  let done = false; let held = false;
  const ipc = vi.fn((command: string, args?: unknown) => {
    if (command === 'setup_example') return pair;
    if (command === 'agent_open') return { ...pair, documentId: sample, name: 'web-reader', agent: changed ? 'Edited instructions' : crlf ? pair.agent.replaceAll('\n', '\r\n') : pair.agent };
    if (command === 'agent_run') { if (unreadable) throw Error('lost submission'); return rejected ? { ...idle, error: 'session' } : { ...idle, state: 'running', canRun: false, busy: { documentId: sample, name: 'web-reader' } }; }
    if (command === 'agent_run_status') return done
      ? { ...idle, state: 'unknown', busy: held ? { documentId: sample, name: 'web-reader' } : null, canRun: !held }
      : ipc.mock.calls.some(([c]) => c === 'agent_run') ? { ...idle, state: 'running', canRun: false, busy: { documentId: sample, name: 'web-reader' } } : idle;
    throw Error(`Unexpected ${command} ${JSON.stringify(args)}`);
  });
  mockIPC(ipc);
  return { ipc, finish: (busy = false) => { done = true; held = busy; } };
}
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); vi.restoreAllMocks(); localStorage.clear(); });

// A missing one-shot claim would submit twice on StrictMode/reconnect/reopen.
test('first completion submits the sample once, waits for release, then opens its result', async () => {
  const { ipc, finish } = native({ crlf: true }); const onOpen = vi.fn(); armFirstTest();
  const view = renderHook(({ ready }) => useFirstTest({ ready, canOpen: true, sampleDirty: false, onOpen }), { initialProps: { ready: false }, wrapper: StrictMode });
  expect(ipc).not.toHaveBeenCalled();
  view.rerender({ ready: true });
  await waitFor(() => expect(view.result.current).toBe('running'));
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toEqual([['agent_run', { documentId: sample, bundledExample: true, attemptId: expect.any(String) }]]);
  expect(onOpen).not.toHaveBeenCalled();
  finish(true);
  await new Promise(resolve => setTimeout(resolve, 550));
  expect(onOpen).not.toHaveBeenCalled();
  finish();
  await waitFor(() => {
    expect(onOpen).toHaveBeenCalledOnce();
    expect(view.result.current).toBe('unknown');
  });
  view.rerender({ ready: false }); view.rerender({ ready: true });
  view.unmount();
  renderHook(() => useFirstTest({ ready: true, canOpen: true, sampleDirty: false, onOpen }));
  armFirstTest();
  await new Promise(resolve => setTimeout(resolve, 550));
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1);
});

test.each(['submitted', 'existing', 'changed', 'changed-after-run', 'unreadable-after-run', 'raced', 'lost-reply', 'left', 'failed', 'unknown', 'cancelled'] as const)('only current successful owned completion offers the first-pass moment (%s)', async scenario => {
  const existing = scenario === 'existing';
  vi.stubGlobal('isTauri', true); armFirstTest();
  const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
  const passed = { ...idle, source: scenario === 'changed' ? { ...source, agent: 'c'.repeat(64) } : source, state: 'complete', report: { verdict: ['failed', 'unknown', 'cancelled'].includes(scenario) ? scenario : 'passed', cases: [{ id: 'checksum', question: 'Read the checksum', phase: 'Succeeded', answer: 'checksum', completedTools: ['web_fetch'], assertions: [{ id: 'a', kind: 'contains', expected: 'checksum', verdict: 'passed' }], verdict: 'passed' }] } };
  let attemptId: unknown;
  const ipc = vi.fn((command: string, payload?: unknown) => {
    const args = payload as Record<string, unknown> | undefined;
    if (command === 'setup_example') return pair;
    if (command === 'agent_open') {
      const finished = ipc.mock.calls.some(([c]) => c === 'agent_run');
      if (finished && scenario === 'unreadable-after-run') throw Error('Files unreadable');
      return { ...pair, source: finished && scenario === 'changed-after-run' ? { ...source, tests: 'd'.repeat(64) } : source, documentId: sample, name: 'web-reader' };
    }
    if (command === 'agent_run_status') return existing || scenario === 'lost-reply' && ipc.mock.calls.some(([c]) => c === 'agent_run') ? { ...passed, ...(attemptId ? { attemptId } : {}) } : idle;
    if (command === 'agent_run') { attemptId = scenario === 'raced' ? '12345678-1234-1234-1234-123456789abc' : args?.attemptId; if (scenario === 'lost-reply') throw Error('Lost reply'); return { ...passed, ...(attemptId ? { attemptId } : {}) }; }
  });
  mockIPC(ipc); const onOpen = vi.fn(); const onPassed = vi.fn(); const onSource = vi.fn();
  const view = renderHook(() => useFirstTest({ ready: true, canOpen: scenario !== 'left', sampleDirty: false, onOpen, onPassed, onSource }));
  await waitFor(() => expect(view.result.current).toBe(['failed', 'unknown'].includes(scenario) ? scenario : scenario === 'cancelled' ? 'unknown' : 'passed'));
  if (scenario === 'changed-after-run') expect(onSource.mock.lastCall?.[0]?.tests).toBe('d'.repeat(64));
  if (scenario === 'unreadable-after-run') expect(onSource).toHaveBeenLastCalledWith(null);
  expect(onPassed).toHaveBeenCalledTimes(scenario === 'submitted' ? 1 : 0);
  expect(onOpen).toHaveBeenCalledTimes(scenario === 'submitted' || scenario === 'left' ? 0 : 1);
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(existing ? 0 : 1);
});

test('replacement clears a completed hook pass permanently', async () => {
  vi.stubGlobal('isTauri', true); armFirstTest();
  mockIPC(command => {
    if (command === 'setup_example') return pair;
    if (command === 'agent_open') return { ...pair, documentId: sample, name: 'web-reader' };
    if (command === 'agent_run_status') return { ...idle, state: 'complete', report: { verdict: 'passed', cases: [{ id: 'checksum', question: 'Read the checksum', phase: 'Succeeded', answer: 'checksum', completedTools: [], assertions: [{ id: 'a', kind: 'contains', expected: 'checksum', verdict: 'passed' }], verdict: 'passed' }] } };
    throw Error('Unexpected ' + command);
  });
  const view = renderHook(({ retired }) => useFirstTest({ ready: !retired, retired, canOpen: false, sampleDirty: false, onOpen: vi.fn() }), { initialProps: { retired: false } });
  await waitFor(() => expect(view.result.current).toBe('passed'));
  view.rerender({ retired: true });
  expect(view.result.current).toBeUndefined();
  view.rerender({ retired: false });
  expect(view.result.current).toBeUndefined();
});

test('replacement retires the hook verdict and ignores a retired in-flight result', async () => {
  const { ipc, finish } = native(); armFirstTest();
  const onSnapshot = vi.fn(); const onOpen = vi.fn();
  const view = renderHook(({ retired }) => useFirstTest({ ready: !retired, retired, canOpen: true, sampleDirty: false, onOpen, onSnapshot }), { initialProps: { retired: false } });
  await waitFor(() => expect(view.result.current).toBe('running'));
  retireFirstTest(); view.rerender({ retired: true });
  await waitFor(() => expect(view.result.current).toBeUndefined());
  view.rerender({ retired: false }); finish();
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 550)); });
  expect(view.result.current).toBeUndefined();
  expect(onOpen).not.toHaveBeenCalled();
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1);
});

test('setup prepares the connected example before opening its saved bytes', async () => {
  vi.stubGlobal('isTauri', true); armFirstTest();
  let prepared = false;
  const ipc = vi.fn((command: string, args?: unknown) => {
    if (command === 'setup_example') { prepared = (args as { prepare?: boolean })?.prepare === true; return pair; }
    if (command === 'agent_open') return { ...pair, agent: prepared ? pair.agent : 'original wrong model', documentId: sample, name: 'web-reader' };
    if (command === 'agent_run_status') return idle;
    if (command === 'agent_run') return { ...idle, error: 'session' };
  });
  mockIPC(ipc);
  renderHook(() => useFirstTest({ ready: true, canOpen: true, sampleDirty: false, onOpen: vi.fn() }));
  await waitFor(() => expect(ipc.mock.calls.some(([c]) => c === 'agent_run')).toBe(true));
  expect(ipc.mock.calls.find(([c]) => c === 'setup_example')).toEqual(['setup_example', { prepare: true }]);
});
test('transient preparation busy preserves the one-shot and notifies the editor before submission', async () => {
  const { ipc } = native({ rejected: true }); const original = ipc.getMockImplementation()!;
  let attempts = 0; const events: string[] = []; const prepared = vi.fn(() => { events.push('prepared'); });
  ipc.mockImplementation((command, args) => {
    if (command === 'setup_example' && attempts++ === 0) throw 'exampleBusy';
    if (command === 'agent_run') events.push('submitted');
    return original(command, args);
  });
  armFirstTest(); renderHook(() => useFirstTest({ ready: true, canOpen: true, sampleDirty: false, onOpen: vi.fn(), onPrepared: prepared }));
  await waitFor(() => expect(ipc.mock.calls.some(([c]) => c === 'agent_run')).toBe(true), { timeout: 2000 });
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1);
  expect(prepared).toHaveBeenCalledOnce(); expect(events).toEqual(['prepared', 'submitted']);
});
test('observing an existing ready setup or retiring intent never submits a test', async () => {
  const { ipc } = native(); const onOpen = vi.fn();
  const view = renderHook(() => useFirstTest({ ready: true, canOpen: true, sampleDirty: false, onOpen }));
  await act(async () => {});
  expect(ipc).not.toHaveBeenCalled(); view.unmount();
  armFirstTest(); retireFirstTest(); armFirstTest();
  renderHook(() => useFirstTest({ ready: true, canOpen: true, sampleDirty: false, onOpen }));
  await act(async () => {});
  expect(ipc).not.toHaveBeenCalled();
});

test('finishing without navigation ownership updates status without pulling the creator away', async () => {
  const { finish } = native(); const onOpen = vi.fn(); armFirstTest();
  const view = renderHook(({ canOpen }) => useFirstTest({ ready: true, canOpen, sampleDirty: false, onOpen }), { initialProps: { canOpen: true } });
  await waitFor(() => expect(view.result.current).toBe('running'));
  view.rerender({ canOpen: false }); finish();
  await waitFor(() => expect(view.result.current).toBe('unknown'));
  expect(onOpen).not.toHaveBeenCalled();
});

test('a readable rejected submission ends without replay', async () => {
  const { ipc } = native({ rejected: true }); const onOpen = vi.fn(); armFirstTest();
  const view = renderHook(() => useFirstTest({ ready: true, canOpen: true, sampleDirty: false, onOpen }));
  await waitFor(() => expect(onOpen).toHaveBeenCalledOnce());
  view.unmount();
  renderHook(() => useFirstTest({ ready: true, canOpen: true, sampleDirty: false, onOpen }));
  await act(async () => {});
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1);
});

test('edited saved sample is not overwritten or automatically submitted', async () => {
  const { ipc } = native({ changed: true }); const onOpen = vi.fn(); armFirstTest();
  renderHook(() => useFirstTest({ ready: true, canOpen: true, sampleDirty: false, onOpen }));
  await waitFor(() => expect(onOpen).toHaveBeenCalledOnce());
  expect(ipc.mock.calls.some(([c]) => c === 'agent_run' || c === 'agent_save')).toBe(false);
});

test.each([true, false])('an admitted test remains observed through unavailable setup status (recover first: %s)', async recoverFirst => {
  const { ipc, finish } = native(); const onOpen = vi.fn(); armFirstTest();
  const view = renderHook(({ ready }) => useFirstTest({ ready, canOpen: true, sampleDirty: false, onOpen }), { initialProps: { ready: true } });
  await waitFor(() => expect(view.result.current).toBe('running'));
  view.rerender({ ready: false });
  expect(view.result.current).toBe('running');
  if (recoverFirst) view.rerender({ ready: true });
  finish();
  await waitFor(() => expect(view.result.current).toBe('unknown'));
  expect(onOpen).toHaveBeenCalledTimes(recoverFirst ? 1 : 0);
  view.rerender({ ready: true });
  await act(async () => {});
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1);
});

test.each([false, true])('unreadable replies keep watching until sample busy ownership is released (lost submission: %s)', async unreadableSubmission => {
  const { ipc, finish } = native({ unreadable: unreadableSubmission }); const onOpen = vi.fn(); armFirstTest();
  let unreadable = false;
  const reply = ipc.getMockImplementation()!;
  ipc.mockImplementation((command, args) => {
    if (command === 'agent_run_status' && unreadable) throw Error('Lost status');
    return reply(command, args);
  });
  const view = renderHook(() => useFirstTest({ ready: true, canOpen: true, sampleDirty: false, onOpen }));
  await waitFor(() => expect(view.result.current).toBe('running'));
  const reads = ipc.mock.calls.filter(([c]) => c === 'agent_run_status').length;
  unreadable = true; finish(true);
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run_status').length).toBeGreaterThan(reads));
  expect(onOpen).not.toHaveBeenCalled();
  expect(view.result.current).toBe('running');
  unreadable = false;
  const beforeRelease = ipc.mock.calls.filter(([c]) => c === 'agent_run_status').length;
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run_status').length).toBeGreaterThan(beforeRelease));
  expect(onOpen).not.toHaveBeenCalled();
  expect(view.result.current).toBe('running');
  finish();
  await waitFor(() => { expect(onOpen).toHaveBeenCalledOnce(); expect(view.result.current).toBe('unknown'); });
  view.unmount();
  renderHook(() => useFirstTest({ ready: true, canOpen: true, sampleDirty: false, onOpen }));
  await act(async () => {});
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(1);
});

test('unsaved sample edits hold the automatic action and storage failure never admits submission', async () => {
  const { ipc } = native(); armFirstTest();
  const view = renderHook(() => useFirstTest({ ready: true, canOpen: true, sampleDirty: true, onOpen: vi.fn() }));
  await act(async () => {}); expect(ipc).not.toHaveBeenCalled(); view.unmount();
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw Error('storage unavailable'); });
  renderHook(() => useFirstTest({ ready: true, canOpen: true, sampleDirty: false, onOpen: vi.fn() }));
  await act(async () => {});
  expect(ipc.mock.calls.some(([c]) => c === 'agent_run')).toBe(false);
});
