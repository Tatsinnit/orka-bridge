import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, expect, test, vi } from 'vitest';
import { chatCall, isChatSnapshot, type ChatSnapshot } from './chat-api';
const id = '9b2b93c4-375c-4815-9717-39852a96e411';
const idle: ChatSnapshot = { documentId: id, state: 'ready', busy: false, canSend: true, versions: [], error: null };
const turn = { message: 'Hello', answer: null, phase: 'Pending', tools: [], contextRetained: false, terminationUnconfirmed: false, stopUnconfirmed: false };
afterEach(() => { clearMocks(); vi.unstubAllGlobals(); });
test.each(['agent_chat_status', 'agent_chat_send', 'agent_chat_reconnect', 'agent_chat_stop', 'agent_chat_new'] as const)('uses document-scoped %s without resource authority', async command => {
  vi.stubGlobal('isTauri', true); const calls: unknown[] = [];
  mockIPC((name, args) => { calls.push([name, args]); return idle; });
  expect(await chatCall(command, id, command === 'agent_chat_send' ? 'Hello' : undefined)).toEqual({ ok: true, snapshot: idle });
  expect(calls).toEqual([[command, { documentId: id, ...(command === 'agent_chat_send' ? { message: 'Hello' } : {}) }]]);
});
test('accepts native versions and persistent interrupted-turn notices without a history cap', () => {
  expect(isChatSnapshot({ ...idle, versions: [{ turns: [{ ...turn, tools: [{ name: 'web_fetch', callId: 'call-1', phase: 'Succeeded' }], terminationUnconfirmed: true }] }] })).toBe(true);
  expect(isChatSnapshot({ ...idle, versions: Array.from({ length: 60 }, () => ({ turns: [turn] })) })).toBe(true);
});
test.each([
  null, { ...idle, state: 'stopped' }, { ...idle, busy: 'false' }, { ...idle, canSend: 'yes' },
  { ...idle, error: 'private raw error' }, { ...idle, documentId: 'path/to/file' },
  { ...idle, versions: [{ turns: [{ ...turn, answer: {} }] }] },
  { ...idle, versions: [{ turns: [{ ...turn, tools: [{ name: 'tool', phase: 7, callId: '1' }] }] }] },
  { ...idle, versions: [{ turns: [{ ...turn, contextRetained: undefined }] }] },
])('rejects unsupported native snapshot %#', value => { expect(isChatSnapshot(value)).toBe(false); });
test('never accepts a different document reply', async () => {
  vi.stubGlobal('isTauri', true); mockIPC(() => ({ ...idle, documentId: 'a7f34ba2-b9bd-4b55-b69d-f0a84a1bf528' }));
  expect((await chatCall('agent_chat_status', id)).ok).toBe(false);
});
test.each(['busy', 'source', 'model', 'contextUnavailable', 'invalidMessage', 'connection'] as const)('translates %s errors without leaking diagnostics', async error => {
  vi.stubGlobal('isTauri', true); mockIPC(() => Promise.reject(error));
  const result = await chatCall('agent_chat_send', id, 'Keep');
  expect(result.ok).toBe(false); if (!result.ok) { expect(result.message.length).toBeGreaterThan(10); expect(result.uncertain).not.toBe(true); }
});
test('hides unknown native exception contents', async () => {
  vi.stubGlobal('isTauri', true); mockIPC(() => Promise.reject('secret-path-and-token'));
  const result = await chatCall('agent_chat_status', id);
  expect(result.ok).toBe(false); expect(JSON.stringify(result)).not.toContain('secret-path-and-token');
  if (!result.ok) expect(result.uncertain).toBe(true);
});
test('browser preview cannot dispatch or claim a native conversation', async () => {
  vi.stubGlobal('isTauri', false);
  expect(await chatCall('agent_chat_status', id)).toEqual({ ok: false, message: 'Chat is available only in the native app.' });
});
