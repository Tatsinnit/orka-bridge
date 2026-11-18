import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, renderHook, screen } from '@testing-library/react';
import { useCallback, useState } from 'react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import type { SendStatus, SendView } from './send-api';
import { useSendSettlement } from './use-send-settlement';

const documentId = '9b2b93c4-375c-4815-9717-39852a96e411';
const otherDocument = '22345678-1234-1234-1234-123456789abc';
const args = { documentId, targetId: '00000000-0000-4000-8000-000000000002', providerId: '00000000-0000-4000-8000-000000000003', expectedSource: { agent: 'a'.repeat(64), tests: 'b'.repeat(64) } };
const running: SendView = { documentId, id: '00000000-0000-4000-8000-000000000004', targetId: args.targetId, providerId: args.providerId, targetName: null, source: null, model: null, agentName: null, state: 'running', stage: 'capture', acknowledged: [], pending: null, report: null, error: null, storageError: false };
const terminal: SendView = { ...running, state: 'unknown', stage: 'stopped' };
const other: SendView = { ...running, documentId: otherDocument, id: '00000000-0000-4000-8000-000000000005' };
const status = (view: SendView): SendStatus => ({ documentId: view.documentId, view, busy: view.state === 'running' ? { documentId: view.documentId, name: 'reader' } : null });
function deferred<T>() {
  let resolve!: (value: T) => void; let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const tick = (milliseconds = 500) => act(async () => { await vi.advanceTimersByTimeAsync(milliseconds); });
beforeEach(() => { vi.useFakeTimers(); vi.stubGlobal('isTauri', true); });
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });

test('the owner observes the originating attempt even when its child unmounts before admission returns', async () => {
  const admission = deferred<SendStatus>(); const settled = vi.fn();
  const ipc = vi.fn((command: string) => command === 'send_start' ? admission.promise : status(terminal));
  mockIPC(ipc);
  function Owner({ child }: { child: boolean }) {
    const start = useSendSettlement(settled);
    return child ? <button onClick={() => { void start(args); }}>Send</button> : null;
  }
  const view = render(<Owner child />);
  fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  view.rerender(<Owner child={false} />);
  await act(async () => { admission.resolve(status(running)); });
  await tick();
  expect(settled).toHaveBeenCalledTimes(1);
  expect(ipc.mock.calls).toEqual([['send_start', args], ['send_status', { documentId }]]);
});

test.each(['failed', 'cancelled', 'unknown'] as const)('a fast %s admission invalidates once per attempt, not on initial mount or duplicate replies', async verdict => {
  const view: SendView = verdict === 'unknown' ? terminal : { ...running, state: 'complete', stage: 'done', report: verdict === 'cancelled'
    ? { verdict, cases: [{ id: 'first', question: 'Read', phase: 'Cancelled', answer: null, completedTools: [], assertions: [], verdict }] }
    : { verdict, cases: [{ id: 'first', question: 'Read', phase: 'Succeeded', answer: 'wrong', completedTools: [], assertions: [{ id: 'answer', kind: 'contains', expected: 'page', verdict }], verdict }] } };
  let reply = view;
  const ipc = vi.fn(() => status(reply)); mockIPC(ipc);
  const settled = vi.fn(); const owner = renderHook(() => useSendSettlement(settled));
  await tick(1000);
  expect(ipc).not.toHaveBeenCalled();
  await act(async () => { await owner.result.current(args); });
  expect(settled).toHaveBeenCalledTimes(1);
  await act(async () => { await owner.result.current(args); });
  await tick(1500);
  expect(settled).toHaveBeenCalledTimes(1);
  reply = { ...view, id: other.id };
  await act(async () => { await owner.result.current(args); });
  expect(settled).toHaveBeenCalledTimes(2);
});

test('status reads never overlap, retry read errors without resubmitting, and stop after settlement', async () => {
  const read = deferred<SendStatus>(); let reads = 0;
  const ipc = vi.fn((command: string) => command === 'send_start' ? status(running) : ++reads === 1 ? read.promise : status(terminal));
  mockIPC(ipc); const settled = vi.fn(); const owner = renderHook(() => useSendSettlement(settled));
  await act(async () => { await owner.result.current(args); });
  await tick(2000);
  expect(reads).toBe(1);
  await act(async () => { read.reject(Error('temporarily unreadable')); });
  expect(settled).not.toHaveBeenCalled();
  await tick();
  expect(settled).toHaveBeenCalledTimes(1);
  await tick(1500);
  expect(reads).toBe(2);
  expect(ipc.mock.calls.filter(([command]) => command === 'send_start')).toHaveLength(1);
});

test.each(['rejected', 'null', 'foreign-busy', 'mismatched-origin'] as const)('a %s start retains the accepted attempt observer', async mode => {
  let starts = 0;
  mockIPC(command => {
    if (command === 'send_start' && starts++ === 0) return status(running);
    if (command === 'send_start') {
      if (mode === 'rejected') throw Error('rejected');
      if (mode === 'null') return { documentId, view: null, busy: null };
      if (mode === 'mismatched-origin') return status(other);
      return { ...status(terminal), busy: { documentId: otherDocument, name: 'helper' } };
    }
    return status(terminal);
  });
  const settled = vi.fn(); const owner = renderHook(() => useSendSettlement(settled));
  await act(async () => {
    await owner.result.current(args);
    await owner.result.current(args).catch(() => undefined);
  });
  expect(settled).not.toHaveBeenCalled();
  await tick();
  expect(settled).toHaveBeenCalledTimes(1);
});

test('a later rejected request cannot fence an earlier valid admission reply', async () => {
  const first = deferred<SendStatus>(); let starts = 0;
  mockIPC(command => {
    if (command === 'send_start') { if (starts++ === 0) return first.promise; throw Error('later request rejected'); }
    return status(terminal);
  });
  const settled = vi.fn(); const owner = renderHook(() => useSendSettlement(settled));
  let pending!: Promise<SendStatus>;
  await act(async () => {
    pending = owner.result.current(args);
    await owner.result.current({ ...args, documentId: otherDocument }).catch(() => undefined);
    first.resolve(status(running)); await pending;
  });
  await tick();
  expect(settled).toHaveBeenCalledTimes(1);
});

test('an earlier request admitted after a later Send settles still notifies and refreshes on completion', async () => {
  const admission = deferred<SendStatus>(); const settled = vi.fn();
  let firstView: SendView | null = null; let secondView = other;
  mockIPC((command, payload) => {
    const request = payload as { documentId: string };
    // Native awaits document_folder before admission: A's lookup can outlast all of B's Send.
    if (command === 'send_start') return request.documentId === documentId ? admission.promise : status(secondView);
    const view = request.documentId === documentId ? firstView : secondView;
    return { documentId: request.documentId, view, busy: view?.state === 'running' ? { documentId: view.documentId, name: 'reader' } : null };
  });
  const owner = renderHook(() => {
    const [placesRevision, setPlacesRevision] = useState(0);
    const onSettled = useCallback(() => { settled(); setPlacesRevision(value => value + 1); }, []);
    return { start: useSendSettlement(onSettled), placesRevision };
  });
  let firstStart!: Promise<SendStatus>;
  await act(async () => {
    firstStart = owner.result.current.start(args);
    await owner.result.current.start({ ...args, documentId: otherDocument });
  });
  secondView = { ...other, state: 'unknown', stage: 'stopped' };
  await tick();
  expect(settled).toHaveBeenCalledTimes(1);
  expect(owner.result.current.placesRevision).toBe(1);
  // B is already terminal before A reaches native admission; the native owner never overlaps Sends.
  await act(async () => { firstView = running; admission.resolve(status(firstView)); await firstStart; });
  await tick();
  expect(settled).toHaveBeenCalledTimes(1);
  expect(owner.result.current.placesRevision).toBe(1);
  firstView = { ...running, state: 'complete', stage: 'done', report: { verdict: 'failed', cases: [{ id: 'first', question: 'Read', phase: 'Succeeded', answer: 'wrong', completedTools: [], assertions: [{ id: 'answer', kind: 'contains', expected: 'page', verdict: 'failed' }], verdict: 'failed' }] } };
  await tick();
  expect.soft(settled).toHaveBeenCalledTimes(2);
  expect(owner.result.current.placesRevision).toBe(2);
  await tick(1500);
  expect(settled).toHaveBeenCalledTimes(2);
  expect(vi.getTimerCount()).toBe(0);
});

test('a delayed older admission reply retains observation of both actual Send IDs', async () => {
  const first = deferred<SendStatus>(); const reads: unknown[] = [];
  let secondView = other;
  mockIPC((command, payload) => {
    const request = payload as { documentId: string };
    if (command === 'send_start') return request.documentId === documentId ? first.promise : status(other);
    reads.push(payload); return status(request.documentId === documentId ? terminal : secondView);
  });
  const settled = vi.fn(); const owner = renderHook(() => useSendSettlement(settled));
  await act(async () => {
    const pending = owner.result.current(args);
    // Native A finished before B was admitted, but A's Running reply was delayed.
    await owner.result.current({ ...args, documentId: otherDocument });
    first.resolve(status(running)); await pending;
  });
  await tick();
  expect(settled).toHaveBeenCalledTimes(1);
  expect(reads).toEqual([{ documentId: otherDocument }, { documentId }]);
  secondView = { ...other, state: 'unknown', stage: 'stopped' };
  await tick();
  expect(settled).toHaveBeenCalledTimes(2);
  expect(reads).toEqual([{ documentId: otherDocument }, { documentId }, { documentId: otherDocument }]);
});

test('an in-flight old status reply settles only its own attempt and never overlaps newer reads', async () => {
  const firstRead = deferred<SendStatus>(); const reads: unknown[] = [];
  let secondView = other;
  mockIPC((command, payload) => {
    const request = payload as { documentId: string };
    if (command === 'send_start') return status(request.documentId === documentId ? running : other);
    reads.push(payload);
    return request.documentId === documentId ? firstRead.promise : status(secondView);
  });
  const settled = vi.fn(); const owner = renderHook(() => useSendSettlement(settled));
  await act(async () => { await owner.result.current(args); }); await tick();
  expect(reads).toEqual([{ documentId }]);
  await act(async () => { await owner.result.current({ ...args, documentId: otherDocument }); });
  await tick(1000);
  expect(reads).toEqual([{ documentId }]);
  await act(async () => { firstRead.resolve(status(terminal)); });
  expect(settled).toHaveBeenCalledTimes(1);
  await tick();
  expect(settled).toHaveBeenCalledTimes(1);
  expect(reads).toEqual([{ documentId }, { documentId: otherDocument }]);
  secondView = { ...other, state: 'unknown', stage: 'stopped' };
  await tick();
  expect(settled).toHaveBeenCalledTimes(2);
});

test('a late same-document admission is invalidated by replacement without adopting its verdict or losing ID deduplication', async () => {
  const first = deferred<SendStatus>(); let starts = 0;
  let replacement = { ...running, id: other.id }; let reply = replacement;
  const ipc = vi.fn((command: string) => command === 'send_start' ? starts++ === 0 ? first.promise : status(reply) : status(replacement));
  mockIPC(ipc);
  const settled = vi.fn(); const owner = renderHook(() => useSendSettlement(settled));
  await act(async () => {
    const pending = owner.result.current(args);
    await owner.result.current(args);
    first.resolve(status(running)); await pending;
  });
  expect(settled).not.toHaveBeenCalled();
  await tick();
  // Only the superseded ID settles: its replacement is still Running.
  expect(settled).toHaveBeenCalledTimes(1);
  replacement = { ...replacement, state: 'unknown', stage: 'stopped' };
  await tick();
  expect(settled).toHaveBeenCalledTimes(2);
  const reads = ipc.mock.calls.filter(([command]) => command === 'send_status').length;
  await act(async () => {
    reply = running; await owner.result.current(args);
    reply = replacement; await owner.result.current(args);
  });
  await tick(1500);
  expect(settled).toHaveBeenCalledTimes(2);
  expect(ipc.mock.calls.filter(([command]) => command === 'send_status')).toHaveLength(reads);
  expect(vi.getTimerCount()).toBe(0);
});

test('a failed read leaves its identity pending and proceeds serially through the same polling round', async () => {
  const read = deferred<SendStatus>(); const reads: unknown[] = []; let firstReads = 0;
  const ipc = vi.fn((command: string, payload: unknown) => {
    const request = payload as { documentId: string };
    if (command === 'send_start') return status(request.documentId === documentId ? running : other);
    reads.push(payload);
    return request.documentId === documentId ? ++firstReads === 1 ? read.promise : status(terminal)
      : status({ ...other, state: 'unknown', stage: 'stopped' });
  });
  mockIPC(ipc); const settled = vi.fn(); const owner = renderHook(() => useSendSettlement(settled));
  await act(async () => {
    await owner.result.current(args);
    await owner.result.current({ ...args, documentId: otherDocument });
  });
  await tick(1500);
  expect(reads).toEqual([{ documentId }]);
  await act(async () => { read.reject(Error('temporarily unreadable')); });
  expect(settled).toHaveBeenCalledTimes(1);
  expect(reads).toEqual([{ documentId }, { documentId: otherDocument }]);
  await tick(499);
  expect(reads).toHaveLength(2);
  await tick(1);
  expect(settled).toHaveBeenCalledTimes(2);
  expect(reads).toEqual([{ documentId }, { documentId: otherDocument }, { documentId }]);
  expect(ipc.mock.calls.filter(([command]) => command === 'send_start')).toHaveLength(2);
  expect(vi.getTimerCount()).toBe(0);
});

test('App unmount during a polling round fences the read and stops before another owned identity', async () => {
  const read = deferred<SendStatus>(); const reads: unknown[] = [];
  mockIPC((command, payload) => {
    const request = payload as { documentId: string };
    if (command === 'send_start') return status(request.documentId === documentId ? running : other);
    reads.push(payload); return read.promise;
  });
  const settled = vi.fn(); const owner = renderHook(() => useSendSettlement(settled));
  await act(async () => {
    await owner.result.current(args);
    await owner.result.current({ ...args, documentId: otherDocument });
  });
  await tick();
  expect(reads).toEqual([{ documentId }]);
  owner.unmount();
  await act(async () => { read.resolve(status(terminal)); });
  await tick(1500);
  expect(settled).not.toHaveBeenCalled();
  expect(reads).toEqual([{ documentId }]);
  expect(vi.getTimerCount()).toBe(0);
});

test.each(['same-attempt', 'replacement'] as const)('a null status stays unknown until an authoritative %s reply', async mode => {
  let reads = 0;
  mockIPC(command => command === 'send_start' ? status(running) : ++reads === 1 ? { documentId, view: null, busy: null }
    : status(mode === 'same-attempt' ? terminal : { ...running, id: other.id }));
  const settled = vi.fn(); const owner = renderHook(() => useSendSettlement(settled));
  await act(async () => { await owner.result.current(args); });
  await tick(); expect(settled).not.toHaveBeenCalled();
  await tick(); expect(settled).toHaveBeenCalledTimes(1);
  await tick(1500); expect(reads).toBe(2);
});

test.each(['before-admission', 'before-poll', 'during-poll'] as const)('App unmount %s cancels observation and ignores late replies', async phase => {
  const admission = deferred<SendStatus>(); const read = deferred<SendStatus>(); let reads = 0;
  mockIPC(command => {
    if (command === 'send_start') return phase === 'before-admission' ? admission.promise : status(running);
    reads++; return read.promise;
  });
  const settled = vi.fn(); const owner = renderHook(() => useSendSettlement(settled));
  let pending!: Promise<SendStatus>;
  await act(async () => { pending = owner.result.current(args); if (phase !== 'before-admission') await pending; });
  if (phase === 'during-poll') { await tick(); expect(reads).toBe(1); }
  owner.unmount();
  await act(async () => { admission.resolve(status(terminal)); read.resolve(status(terminal)); await pending; });
  await tick(1500);
  expect(settled).not.toHaveBeenCalled();
  expect(reads).toBe(phase === 'during-poll' ? 1 : 0);
  expect(vi.getTimerCount()).toBe(0);
});
