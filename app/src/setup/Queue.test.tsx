import { openLocalSetup } from './test-navigation';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../App';

const pair = { agent: '---\nname: web-reader\ndescription: Reads a page\n---\nFetch before answering.', tests: 'The checksum check.' };
const doc = '9b2b93c4-375c-4815-9717-39852a96e411';
type Sim = { selected: 'localVm' | null; localVm: string; model: 'locked' | 'available'; modelSupported: boolean | undefined; unreadable: boolean; connect: 'ok' | 'fail' | 'hang'; remove: 'ok' | 'fail' | 'hang' };
// A mutable native stand-in. Tests flip its status the way a real download and startup would.
function native(localVm = 'installing', extra: Partial<Sim> = {}) {
  vi.stubGlobal('isTauri', true);
  const sim: Sim = { selected: 'localVm', localVm, model: 'locked', modelSupported: true, unreadable: false, connect: 'ok', remove: 'ok', ...extra };
  const ipc = vi.fn((command: string): unknown => {
    switch (command) {
      case 'setup_status': if (sim.unreadable) throw Error('unreadable');
        return { selected: sim.selected, localVm: sim.localVm, model: sim.model, ...(sim.modelSupported === undefined ? {} : { modelSupported: sim.modelSupported }), workspaceProgress: sim.localVm === 'installing'
          ? { phase: 'materializing', bytes: 1000, totalBytes: 4000, elapsedMs: 1000, remainingMs: 5000 } : null };
      case 'setup_cancel_local': sim.localVm = 'cancelled'; return { selected: sim.selected, localVm: sim.localVm, model: 'locked' };
      case 'setup_select_local': sim.selected = 'localVm'; sim.localVm = 'installing'; return { selected: 'localVm', localVm: 'installing', model: 'locked' };
      case 'setup_change_local': sim.selected = null; sim.localVm = 'idle'; sim.model = 'locked'; return { selected: null, localVm: 'idle', model: 'locked' };
      case 'setup_connect_model': if (sim.connect === 'hang') return new Promise(() => {}); if (sim.connect === 'fail') throw { kind: 'modelConnect' }; return { ready: true };
      case 'setup_remove_model': if (sim.remove === 'hang') return new Promise(() => {}); if (sim.remove === 'fail') throw { kind: 'modelStore' }; return { ready: false };
      case 'setup_requirements': return null;
      case 'cluster_contexts': return [{ id: '00000000-0000-4000-8000-000000000001', name: 'team-dev', current: true }];
      case 'cluster_discover': return [];
      case 'setup_example': return pair;
      case 'agent_open': return { ...pair, documentId: doc, name: 'web-reader' };
      case 'agent_save': return { ...pair, documentId: doc, name: 'web-reader' };
      case 'agent_run_status': return { documentId: doc, busy: null, state: 'idle', canRun: true, report: null, acknowledged: [], pending: null, error: null };
      default: throw Error('Unexpected command: ' + command);
    }
  });
  mockIPC(ipc);
  const connects = () => ipc.mock.calls.filter(([command]) => command === 'setup_connect_model').length;
  return { ipc, sim, connects };
}
const settle = (ms = 1300) => act(async () => { await new Promise(resolve => setTimeout(resolve, ms)); });
const wait = { timeout: 3000 };
beforeEach(() => { localStorage.setItem('bridge.welcomeSeen.v1', '1'); Object.defineProperty(HTMLDivElement.prototype, 'showPopover', { configurable: true, value() { this.style.display = 'grid'; } }); });
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); Reflect.deleteProperty(HTMLDivElement.prototype, 'showPopover'); });

// One explicit native observation: the window-focus event is Setup's own refresh path. The setup_status call count proves the
// refresh was requested, and a macrotask flush lets its reply and the resulting React updates land before anything is asserted.
const statusReads = (ipc: { mock: { calls: unknown[][] } }) => ipc.mock.calls.filter(([command]) => command === 'setup_status').length;
async function observe(ipc: { mock: { calls: unknown[][] } }) {
  const before = statusReads(ipc);
  await act(async () => { window.dispatchEvent(new Event('focus')); });
  await waitFor(() => expect(statusReads(ipc)).toBeGreaterThan(before));
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
}

test('a choice during transfer connects exactly once at readiness, with no second click and no disabled status buttons', async () => {
  const { ipc, sim, connects } = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  const model = (await screen.findByRole('heading', { name: 'Connect the AI' })).closest('section');
  await user.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect(model?.closest('.setup-column')).toBeTruthy();
  expect(screen.getByText('Queued', { selector: '.pill' }).closest('button')).toBeNull();
  expect(screen.getByText('Queued', { selector: '.step-result' })).toBeTruthy();
  expect(connects()).toBe(0);
  // Orka can be ready before the model unlocks: hold, neither fire nor forget.
  sim.localVm = 'ready';
  await observe(ipc);
  expect(within(screen.getByRole('navigation')).getByText('AI unavailable')).toBeTruthy();
  expect(connects()).toBe(0);
  expect(screen.getByText('Queued', { selector: '.pill' })).toBeTruthy();
  sim.model = 'available';
  await observe(ipc);
  await screen.findByText('Connected', { selector: '.model-state .pill' });
  expect(connects()).toBe(1);
  expect(screen.getByRole('heading', { name: 'Connect the AI' }).closest('section')).toBe(model);
  await observe(ipc);
  expect(connects()).toBe(1);
  expect(within(screen.getByRole('contentinfo')).getByText('Local cluster ready')).toBeTruthy();
  expect(screen.getAllByRole('button').filter(button => button.hasAttribute('disabled') && /connect|queued|removing/i.test(button.textContent ?? ''))).toEqual([]);
});

test.each([false, undefined])('a platform without native model support (%s) offers no early choice and never reaches for credentials', async modelSupported => {
  const { ipc, sim, connects } = native('installing', { modelSupported }); render(<App />); await openLocalSetup();
  await screen.findAllByText('Getting your workspace ready', {}, wait);
  expect(screen.queryByRole('button', { name: 'Connect the AI' })).toBeNull();
  expect(screen.getAllByText('Waiting').length).toBeGreaterThan(0);
  expect(screen.queryByText('Pick any time')).toBeNull();
  sim.localVm = 'ready'; await observe(ipc);
  expect(screen.getByRole('button', { name: 'Connect the AI' }).matches(':disabled')).toBe(true);
  await settle(); expect(connects()).toBe(0);
});

test('readiness with an unlocked model connects once immediately', async () => {
  const { sim, connects } = native('starting'); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  sim.localVm = 'ready'; sim.model = 'available';
  await screen.findByText('Connected', { selector: '.model-state .pill' }, wait);
  await settle(); expect(connects()).toBe(1);
});

test('an unreadable observation holds the intent without firing or clearing it', async () => {
  const { sim, connects } = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  sim.unreadable = true;
  await within(screen.getByRole('navigation')).findByText('Local cluster unavailable', {}, wait);
  sim.localVm = 'ready'; sim.model = 'available';
  await settle(); expect(connects()).toBe(0);
  expect(screen.getByText('Queued', { selector: '.pill' })).toBeTruthy();
  expect(screen.queryByText('Connected', { selector: '.model-state .pill' })).toBeNull();
  sim.unreadable = false;
  await screen.findByText('Connected', { selector: '.model-state .pill' }, wait);
  await settle(); expect(connects()).toBe(1);
});

test('the hidden Setup keeps polling and finishes an already-submitted intent from Designer', async () => {
  const { sim, connects } = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  sim.localVm = 'ready'; sim.model = 'available';
  await waitFor(() => expect(connects()).toBe(1), wait);
  await within(screen.getByRole('contentinfo')).findByText('Local cluster ready', {}, wait);
  await settle(); expect(connects()).toBe(1);
});

test('cancelling ends the attempt: a later readiness does not connect and a fresh attempt needs fresh consent', async () => {
  const { sim, connects } = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await user.click(screen.getByRole('button', { name: 'Cancel' }));
  await screen.findByRole('button', { name: 'Start again' });
  expect(screen.queryByText('Queued')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Connect the AI' })).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Start again' }));
  expect(await screen.findByRole('button', { name: 'Connect the AI' })).toBeTruthy();
  sim.localVm = 'ready'; sim.model = 'available';
  expect(await screen.findByRole('button', { name: 'Connect the AI' }, wait)).toBeTruthy();
  await settle(); expect(connects()).toBe(0);
});

test('a failed attempt drops the intent and a retry reaching readiness does not connect', async () => {
  const { sim, connects } = native(); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  sim.localVm = 'failed';
  await screen.findByRole('alert', {}, wait);
  expect(screen.queryByText('Queued')).toBeNull();
  sim.localVm = 'ready'; sim.model = 'available';
  expect(await screen.findByRole('button', { name: 'Connect the AI' }, wait)).toBeTruthy();
  await settle(); expect(connects()).toBe(0);
});

test('a queued connection that fails is not retried without an explicit action', async () => {
  const { sim, connects } = native('starting', { connect: 'fail' }); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  sim.localVm = 'ready'; sim.model = 'available';
  await screen.findByText('Bridge could not confirm the AI connection. Check again to reconnect.', {}, wait);
  await settle(); expect(connects()).toBe(1);
  sim.connect = 'ok';
  await user.click(screen.getByRole('button', { name: 'Check again' }));
  await screen.findByText('Connected', { selector: '.model-state .pill' });
  expect(connects()).toBe(2);
});

test('using a cluster clears the held intent even though the view stays mounted', async () => {
  const { ipc, sim, connects } = native();
  const original = ipc.getMockImplementation();
  ipc.mockImplementation(command => {
    if (command === 'cluster_discover') return [{ id: '00000000-0000-4000-8000-000000000002', context_id: '00000000-0000-4000-8000-000000000001', context_name: 'team-dev', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'ghcr.io/orka:0.9.4' }];
    if (command === 'cluster_use') return { status: 'connected', providers: [{ id: '00000000-0000-4000-8000-000000000003', name: 'team-model', ready: true }], failure: null };
    return original?.(command);
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  sim.localVm = 'ready';
  await observe(ipc);
  expect(within(screen.getByRole('navigation')).getByText('AI unavailable')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  await user.click(await screen.findByRole('button', { name: 'kubectl context' }));
  await user.click(screen.getByRole('button', { name: 'team-dev' }));
  await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  await screen.findAllByText('Access confirmed');
  sim.model = 'available';
  await observe(ipc); expect(connects()).toBe(0);
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await openLocalSetup(user);
  await user.click(screen.getByRole('button', { name: 'Use this computer' }));
  expect(await screen.findByRole('button', { name: 'Connect the AI' })).toBeTruthy();
  await observe(ipc); expect(connects()).toBe(0);
});

test.each(['notIntegrated', 'cancelled', 'closed', 'closing', 'idle'])('nothing can be queued while the workspace is %s', async localVm => {
  const { connects } = native(localVm); render(<App />); await openLocalSetup();
  await screen.findByRole('region', { name: 'Local cluster setup' });
  expect(screen.queryByRole('button', { name: 'Connect the AI' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Connect the AI' })).toBeNull();
  expect(screen.queryByText('Queued')).toBeNull();
  expect(connects()).toBe(0);
});

test('the place label stays This computer without inventing selection or readiness', async () => {
  const { sim } = native('installing', { selected: null }); sim.localVm = 'idle';
  let release!: () => void; const gate = new Promise<void>(resolve => { release = resolve; });
  const ipc = vi.fn(async (command: string) => {
    if (command === 'setup_status') { await gate; if (sim.unreadable) throw Error('unreadable'); return { selected: sim.selected, localVm: sim.localVm, model: 'locked' }; }
    if (command === 'setup_requirements') return null;
    throw Error('Unexpected command: ' + command);
  });
  mockIPC(ipc);
  render(<App />);
  const chip = () => document.querySelector('.host-chip')!;
  expect(chip().querySelector('.host-label')?.textContent).toBe('Checking…');
  await act(async () => { release(); });
  await waitFor(() => expect(chip().querySelector('.host-label')?.textContent).toBe('This computer'));
  expect(chip().querySelector('.live-dot')?.className).not.toContain('ready');
  expect(chip().tagName).toBe('SPAN');
  expect(within(screen.getByRole('navigation')).queryByRole('button', { name: /computer|place/i })).toBeNull();
  // A valid saved choice restored after relaunch is "This computer"; an unreadable read afterwards is not "no selection".
  sim.selected = 'localVm'; sim.localVm = 'starting';
  await waitFor(() => expect(chip().querySelector('.host-label')?.textContent).toBe('This computer'), wait);
  sim.unreadable = true;
  await waitFor(() => expect(chip().querySelector('.host-label')?.textContent).toBe('Status unavailable'), wait);
  expect(chip().textContent).not.toContain('No cluster selected');
});

test('an edit marks the first test stale immediately without putting the ready workspace back in setup', async () => {
  native('ready', { model: 'available' });
  const snapshot = { documentId: doc, busy: null, state: 'complete', canRun: true, report: { verdict: 'passed', cases: [{ id: 'a', question: 'What did the page say?', phase: 'Succeeded', answer: 'ok', completedTools: [], assertions: [{ id: 'a1', kind: 'contains', expected: 'ok', verdict: 'passed' }], verdict: 'passed' }] }, acknowledged: [], pending: null, error: null };
  const user = userEvent.setup();
  mockIPC(command => command === 'agent_run_status' ? snapshot : command === 'setup_status' ? { selected: 'localVm', localVm: 'ready', model: 'available' }
    : command === 'setup_connect_model' ? { ready: true } : command === 'setup_requirements' ? null
    : command === 'setup_example' ? pair : command === 'agent_open' ? { ...pair, documentId: doc, name: 'web-reader' }
    : command === 'agent_save' ? new Promise(() => {}) : Promise.reject({ kind: 'unavailable' }));
  render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await user.click(await screen.findByRole('button', { name: 'Read its instructions' }));
  await within(screen.getByRole('contentinfo')).findByText('1 of 1 passed');
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await waitFor(() => expect(within(screen.getByRole('navigation')).getByText('Local cluster ready')).toBeTruthy());
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  await user.click(screen.getByRole('button', { name: 'File' }));
  await user.type(screen.getByRole('textbox', { name: 'Agent file' }), ' edit');
  // No wait for a save or poll: the edit callback itself marks every consumer stale.
  expect(within(screen.getByRole('contentinfo')).getByText('Tests not run since your edit')).toBeTruthy();
  expect(within(screen.getByRole('navigation')).getByText((_, node) => node?.tagName === 'SMALL' && !!node.textContent?.includes('tests need running'))).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(within(screen.getByRole('navigation')).queryByText('Setting up, step 5 of 5')).toBeNull();
  expect(within(screen.getByRole('navigation')).getByText('Tests need running')).toBeTruthy();
  expect(within(screen.getByRole('region', { name: 'Local cluster setup' })).getByText('Tests need running')).toBeTruthy();
});

test('the host stays ready while AI attention and connecting states remain in the setup step', async () => {
  native('ready', { model: 'available', connect: 'hang' }); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  const dots = () => [...document.querySelectorAll('.live-dot')].map(dot => dot.className.replace('live-dot', '').trim());
  await screen.findByRole('button', { name: 'Connect the AI' });
  expect(dots()).toEqual(['ready']);
  expect(screen.getByText('Choose the AI your agent uses').closest('li')?.querySelector('.status-needs-you')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Connect the AI' }));
  await screen.findByText('Connecting', { selector: '.step-result' });
  expect(dots()).toEqual(['ready']);
  expect(screen.getByText('Choose the AI your agent uses').closest('li')?.querySelector('.status-running')).toBeTruthy();
  expect(within(screen.getByRole('contentinfo')).getByText('Connecting the AI…, step 3 of 4')).toBeTruthy();
  expect(within(screen.getByRole('navigation')).getByText('Setting up, step 3 of 4')).toBeTruthy();
});

test('removing the sign-in leaves the computer ready without claiming tests passed', async () => {
  native('ready', { model: 'available', remove: 'hang' }); const user = userEvent.setup(); render(<App />); await openLocalSetup();
  const dots = () => [...document.querySelectorAll('.live-dot')].map(dot => dot.className.replace('live-dot', '').trim());
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await screen.findByText('Connected', { selector: '.model-state .pill' });
  expect(dots()).toEqual(['ready']);
  expect(screen.queryByText('Passed', { selector: '.setup-progress .pill' })).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Remove sign-in' }));
  await user.click(within(screen.getByRole('group', { name: 'Remove sign-in confirmation' })).getByRole('button', { name: 'Remove sign-in' }));
  await screen.findByText('Removing sign-in…');
  expect(dots()).toEqual(['ready']);
});
