import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, expect, test, vi } from 'vitest';
import { isLiveAgents, isPlaces, isSavedFiles, readAgents, readFiles, readPlaces } from './api';
const installation = '12345678-1234-1234-1234-123456789abc';
const complete = { state: 'complete', installation, agents: [], observedAt: 123, observation: null, runActive: false, reason: null };
afterEach(() => { clearMocks(); vi.unstubAllGlobals(); });
test('native reads take no renderer selector, source, namespace or path', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC((command, args) => {
    expect(args).toEqual({});
    return command === 'overview_files' ? { state: 'complete', files: [], runActive: false } : complete;
  });
  expect((await readFiles()).state).toBe('complete'); expect(await readAgents()).toEqual(complete);
});
test('malformed and incomplete metadata is unavailable, not an empty success', async () => {
  vi.stubGlobal('isTauri', true); mockIPC(() => ({ ...complete, observedAt: null }));
  expect((await readAgents()).state).toBe('unavailable');
  expect(isLiveAgents({ ...complete, agents: [{ name: 'agent', namespace: 'ns' }] })).toBe(false);
  expect(isLiveAgents({ ...complete, agents: [{ name: 'x'.repeat(1048577), namespace: 'ns', uid: 'uid' }] })).toBe(false);
  expect(isLiveAgents({ ...complete, observation: { installation, documentId: installation, source: { agent: 'no', tests: 'no' } } })).toBe(false);
  expect(isSavedFiles({ state: 'complete', files: [], runActive: 'false' })).toBe(false);
});
test('supported long declarations, empty optional metadata and large registries are not discarded', () => {
  const file = { documentId: installation, name: 'reader', state: 'readable', source: null, summary: { name: '', description: 'x'.repeat(5000), model: 'model', tools: [] }, reason: null };
  expect(isSavedFiles({ state: 'complete', runActive: false, files: [file] })).toBe(true);
  expect(isSavedFiles({ state: 'complete', runActive: false, files: Array(1025).fill(file) })).toBe(true);
  expect(isLiveAgents({ ...complete, agents: Array(1025).fill({ name: 'agent', namespace: 'ns', uid: 'uid' }) })).toBe(true);
});
const runAgent = { installation, namespace: 'orka-system', agentUid: 'run-uid' };
const document = '9b2b93c4-375c-4815-9717-39852a96e411';
const result = { installation, documentId: document, source: { agent: 'a'.repeat(64), tests: 'b'.repeat(64) }, namespace: 'orka-system', agentName: 'bridge-run-1', agentUid: 'run-uid', observedAt: 5, state: 'complete', report: { verdict: 'failed', passed: 1, total: 3 } };
const saved = { documentId: document, name: 'reader', state: 'readable', source: result.source, summary: null, reason: null, localResult: result };
const history = { state: 'complete', runActive: false, historyState: 'complete', runAgents: [runAgent], files: [saved] };
test('local results, history state and run Agent ownership are validated and normalised', async () => {
  expect(isSavedFiles(history)).toBe(true);
  vi.stubGlobal('isTauri', true); mockIPC(() => history);
  expect(await readFiles()).toEqual(history);
  // Older payloads cannot prove ownership or a last result: conservative defaults, never an invented "none".
  const old = { state: 'complete', runActive: false, files: [{ ...saved, localResult: undefined }] };
  mockIPC(() => old);
  const read = await readFiles();
  expect(read.historyState).toBe('unavailable'); expect(read.runAgents).toEqual([]); expect(read.files[0].localResult).toBeNull();
});
test.each([
  ['unknown history state', { ...history, historyState: 'maybe' }],
  ['run Agents without history', { ...history, historyState: 'unavailable' }],
  ['run Agents not an array', { ...history, runAgents: {} }],
  ['run Agent missing uid', { ...history, runAgents: [{ installation, namespace: 'ns' }] }],
  ['run Agent with a bad installation', { ...history, runAgents: [{ ...runAgent, installation: 'x' }] }],
  ['result for another document', { ...history, files: [{ ...saved, localResult: { ...result, documentId: installation } }] }],
  ['result without counts', { ...history, files: [{ ...saved, localResult: { ...result, report: { verdict: 'passed' } } }] }],
  ['more passed than total', { ...history, files: [{ ...saved, localResult: { ...result, report: { verdict: 'passed', passed: 4, total: 3 } } }] }],
  ['fractional counts', { ...history, files: [{ ...saved, localResult: { ...result, report: { verdict: 'passed', passed: 0.5, total: 1 } } }] }],
  ['result state invented', { ...history, files: [{ ...saved, localResult: { ...result, state: 'invented' } }] }],
  ['result as a string', { ...history, files: [{ ...saved, localResult: 'passed' }] }],
])('malformed local result metadata fails closed: %s', async (_, value) => {
  expect(isSavedFiles(value)).toBe(false);
  vi.stubGlobal('isTauri', true); mockIPC(() => value);
  expect((await readFiles()).state).toBe('unavailable');
});
test('places require coherent scoped identity and complete summary schema', async () => {
  const agent = { name: 'reader', namespace: 'agents', uid: 'uid', agentType: null, model: null, tools: null, record: null };
  const place = { key: 'key', context: 'team', service: 'control/orka', namespace: 'agents', stale: false, observedAt: 123, agents: [agent] };
  const valid = { places: [place], historyState: 'complete', unobserved: [] };
  expect(isPlaces(valid)).toBe(true);
  expect(isPlaces({ ...valid, places: [{ ...place, observedAt: null }] })).toBe(false);
  expect(isPlaces({ ...valid, places: [{ ...place, agents: [{ ...agent, namespace: 'elsewhere' }] }] })).toBe(false);
  expect(isPlaces({ ...valid, places: [{ ...place, agents: [{ ...agent, tools: [1] }] }] })).toBe(false);
  expect(isPlaces({ ...valid, places: [place, place] })).toBe(false);
  expect(isPlaces({ ...valid, unobserved: [{ agentUid: null, verdict: 'passed' }] })).toBe(false);
  vi.stubGlobal('isTauri', true); mockIPC((command, args) => { expect(command).toBe('overview_places'); expect(args).toEqual({}); return valid; });
  expect(await readPlaces()).toEqual(valid);
  mockIPC(() => ({ ...valid, historyState: 'invented' }));
  expect(await readPlaces()).toBeUndefined();
});
test('cancelled local summaries do not make the entire Overview unavailable', () => {
  const observation = { installation, documentId: installation, source: { agent: 'a'.repeat(64), tests: 'b'.repeat(64) }, namespace: 'orka-system', agentName: 'example', agentUid: 'uid', observedAt: 123, state: 'complete', report: { verdict: 'cancelled', passed: 1, total: 2 } };
  expect(isLiveAgents({ ...complete, observation })).toBe(true);
  expect(isLiveAgents({ ...complete, observation: { ...observation, report: { verdict: 'invented', passed: 0, total: 0 } } })).toBe(false);
});
test('a scoped cluster read sends only its remembered key', async () => {
  vi.stubGlobal('isTauri', true);
  const value = { places: [], historyState: 'complete', unobserved: [] };
  mockIPC((command, args) => { expect(command).toBe('overview_places'); expect(args).toEqual({ key: 'remembered-key' }); return value; });
  expect(await readPlaces('remembered-key')).toEqual(value);
});
test('last connection check time does not imply a known agent inventory', () => {
  const row = { key: 'key', context: 'team', service: 'control/orka', namespace: 'agents', stale: true, checkedAt: 123, observedAt: null, agents: [] };
  expect(isPlaces({ places: [row], historyState: 'complete', unobserved: [] })).toBe(true);
  expect(isPlaces({ places: [{ ...row, checkedAt: -1 }], historyState: 'complete', unobserved: [] })).toBe(false);
});
test('native rejection and browser preview never expose diagnostics', async () => {
  vi.stubGlobal('isTauri', true); mockIPC(() => { throw Error('/private/path with credential'); });
  expect(JSON.stringify(await readFiles())).not.toMatch(/private|credential/);
  expect(JSON.stringify(await readAgents())).not.toMatch(/private|credential/);
  vi.stubGlobal('isTauri', false); expect((await readAgents()).state).toBe('unavailable');
});
