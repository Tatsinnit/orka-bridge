import { expect, test } from 'vitest';
import { canQueueModel, infraProgress, setupView } from './progress';
import type { CheckStepState } from './Steps';
import type { SetupStatus, WorkspaceProgress } from './state';

const status = (localVm: SetupStatus['localVm'], extra: Partial<SetupStatus> = {}): SetupStatus =>
  ({ selected: 'localVm', localVm, model: localVm === 'ready' ? 'available' : 'locked', modelSupported: true, ...extra });
const transfer = (bytes: number | null, totalBytes: number | null, remainingMs: number | null = null): WorkspaceProgress =>
  ({ phase: 'materializing', bytes, totalBytes, elapsedMs: 1000, remainingMs });
const starting = (elapsedMs: number): WorkspaceProgress => ({ phase: 'starting', bytes: null, totalBytes: null, elapsedMs, remainingMs: null });

test('one derivation maps native and model state to the four setup steps', () => {
  const step = (s: SetupStatus, model: Parameters<typeof setupView>[1] = {}) => setupView(s, model).line;
  expect(step(status('preparing'))).toBe('Getting the workspace ready');
  expect(step(status('installing'))).toBe('Getting the workspace ready');
  expect(step(status('starting'))).toBe('Starting the workspace');
  expect(step(status('ready'), { model: 'idle' })).toBe('Connect the AI');
  expect(step(status('ready'), { model: 'queued' })).toBe('AI queued');
  expect(step(status('ready'), { model: 'connecting' })).toBe('Connecting the AI…');
  expect(step(status('ready'), { model: 'connected' })).toBe('Workspace ready');
  expect(step(status('ready'), { model: 'connected', check: 'running', initialAutomatic: true })).toBe('Run your first agent');
  expect(setupView(status('ready'), { model: 'connected' }).step).toBe(4);
});

test('status icons follow active work and uncertain evidence without changing setup actions', () => {
  expect(setupView(undefined, { checking: true }).status).toBe('running');
  expect(setupView(undefined).status).toBe('needs-you');
  for (const check of ['stale', 'unknown'] as const) {
    expect(setupView(status('ready'), { model: 'connected', check })).toMatchObject({ status: 'stale', tone: 'stale', done: true, step: 4, action: 'Manage' });
  }
  expect(setupView(status('ready'), { model: 'connected' })).toMatchObject({ status: 'passed', line: 'Workspace ready', done: true, action: 'Manage' });
});

test('the place choice shows no step count', () => {
  for (const s of [status('idle', { selected: null }), status('failed', { selected: null })]) {
    const view = setupView(s);
    expect(view).toMatchObject({ place: 'none', step: 0, line: 'Choose where agents run', detail: 'Choose where agents run' });
    expect(view.line).not.toMatch(/step/i);
  }
});

test('host selection is native: local only when selected, never inferred while checking or unreadable', () => {
  expect(setupView(status('installing')).place).toBe('local');
  expect(setupView(status('idle', { selected: null })).place).toBe('none');
  expect(setupView(undefined, { checking: true })).toMatchObject({ place: 'checking', line: 'Checking the workspace…' });
  expect(setupView(undefined, { checking: false })).toMatchObject({ place: 'unavailable', line: 'Workspace unavailable' });
});

test('test uncertainty and manual runs do not reopen completed workspace setup', () => {
  const done = (check?: CheckStepState) => setupView(status('ready'), { model: 'connected', check });
  for (const check of ['passed', 'failed', 'stale', 'unknown', 'running'] as const) expect(done(check)).toMatchObject({ done: true });
  expect(done()).toMatchObject({ done: true, line: 'Workspace ready' });
  expect(setupView(status('ready'), { model: 'connected', check: 'running', initialAutomatic: true })).toMatchObject({ done: false, step: 4 });
});

test('model and workspace trouble keep their own words and tone', () => {
  expect(setupView(status('ready'), { model: 'failed' })).toMatchObject({ line: 'AI not connected', tone: 'needs-you', step: 3 });
  expect(setupView(status('ready', { model: 'locked' }), { model: 'connected' })).toMatchObject({ line: 'AI not connected' });
  expect(setupView(status('failed'))).toMatchObject({ line: 'Setup failed', tone: 'bad' });
  expect(setupView(status('failed', { workspaceFailure: { operation: 'start', cause: 'unknown', codes: [] } })).step).toBe(2);
  expect(setupView(status('failed', { workspaceFailure: { operation: 'download', cause: 'network', codes: [] } })).step).toBe(2);
  expect(setupView(status('cancelled')).line).toBe('Setup paused');
  expect(setupView(status('closed')).line).toBe('Workspace stopped');
  expect(setupView(status('notIntegrated', { selected: null })).line).toBe('Workspace unavailable');
});

test('only an unfinished local attempt can queue a model choice', () => {
  for (const state of ['preparing', 'installing', 'starting'] as const) expect(canQueueModel(status(state))).toBe(true);
  for (const state of ['idle', 'ready', 'failed', 'cancelled', 'closed', 'closing', 'notIntegrated'] as const) expect(canQueueModel(status(state))).toBe(false);
  expect(canQueueModel(status('installing', { selected: null }))).toBe(false);
  // Native support must be affirmed; an unsupported or unknown platform never offers the early choice.
  expect(canQueueModel(status('installing', { modelSupported: false }))).toBe(false);
  expect(canQueueModel(status('installing', { modelSupported: undefined }))).toBe(false);
  expect(canQueueModel(undefined)).toBe(false);
});

test('transfer bytes drive the bar but measure this file step only', () => {
  const GiB = 1024 ** 3;
  const full = infraProgress(status('installing', { workspaceProgress: transfer(2 * GiB, 2 * GiB) }))!;
  expect(full.value).toBe(1);
  const half = infraProgress(status('installing', { workspaceProgress: transfer(GiB, 2 * GiB) }))!;
  expect(half.value).toBeGreaterThan(0); expect(half.value!).toBeLessThan(full.value!);
  expect(half.text).toBe('Downloading, 1.1 of 2.1 GB');
  expect(infraProgress(status('installing', { workspaceProgress: transfer(null, null) }))).toEqual({ value: null });
  expect(infraProgress(status('installing', { workspaceProgress: null }))!.value).toBeNull();
});

test('remaining time is native, singular and never invented', () => {
  expect(infraProgress(status('installing', { workspaceProgress: transfer(1, 2, 1000) }))!.remaining).toBe('1 second left');
  expect(infraProgress(status('installing', { workspaceProgress: transfer(1, 2, 1001) }))!.remaining).toBe('2 seconds left');
  expect(infraProgress(status('installing', { workspaceProgress: transfer(1, 2, null) }))!.remaining).toBeUndefined();
  expect(infraProgress(status('installing', { workspaceProgress: transfer(1, 2, 0) }))!.remaining).toBeUndefined();
});

test('startup has no fabricated completion percentage or remaining estimate', () => {
  for (const elapsed of [1000, 15000, 30000, 300000]) {
    expect(infraProgress(status('starting', { workspaceProgress: starting(elapsed) }))).toEqual({ value: null, text: `${elapsed / 1000} second${elapsed === 1000 ? '' : 's'} elapsed` });
  }
  expect(infraProgress(status('starting', { workspaceProgress: null }))).toEqual({ value: null });
  for (const state of ['ready', 'idle', 'cancelled', 'failed', 'closing'] as const) expect(infraProgress(status(state))).toBeNull();
});

test('tone: running work is busy blue, waiting for the creator is needs-you, ready only when truly complete', () => {
  const tone = (state: Parameters<typeof setupView>[1], s = status('ready')) => setupView(s, state).tone;
  // Busy, never green, while the app is working.
  for (const model of ['connecting', 'removing'] as const) expect(tone({ model })).toBe('busy');
  expect(tone({ model: 'connected', check: 'running' })).toBe('busy');
  for (const state of ['preparing', 'installing', 'starting'] as const) expect(setupView(status(state)).tone).toBe('busy');
  // Waiting on the creator is amber, not ready; waiting on the machine is neutral.
  expect(tone({ model: 'idle' })).toBe('needs-you');
  expect(tone({ model: 'connected' })).toBe('ready');
  expect(tone({ model: 'connected', check: 'stale' })).toBe('stale');
  expect(tone({ model: 'connected', check: 'unknown' })).toBe('stale');
  expect(tone({ model: 'queued' })).toBe('wait');
  expect(tone({ model: 'idle' }, status('ready', { model: 'locked' }))).toBe('wait');
  expect(tone({}, status('cancelled'))).toBe('wait');
  // Only a finished flow is green; model trouble is red.
  expect(tone({ model: 'connected', check: 'passed' })).toBe('ready');
  expect(tone({ model: 'connected', check: 'failed' })).toBe('bad');
  expect(tone({ model: 'failed' })).toBe('needs-you');
  expect(setupView(status('failed')).tone).toBe('bad');
});
