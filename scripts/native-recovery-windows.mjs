// Hosted-CI renderer fixtures in the real app's WebView2, not native IPC proofs.
// CDP is exposed only on loopback for this child process; no product test hooks.
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';

if (process.platform !== 'win32' || process.env.GITHUB_ACTIONS !== 'true') throw Error('Hosted Windows CI only');
const [executable, output] = process.argv.slice(2).map(value => resolve(value));
if (!executable || !output) throw Error('Expected executable and output directory');
for (const root of [process.env.APPDATA, process.env.LOCALAPPDATA]) {
  if (!root) throw Error('Missing native profile root');
  for (const leaf of ['setup-v1/local-vm', 'local-vm-v1']) {
    if (existsSync(join(root, 'ai.orka.bridge', leaf))) throw Error('Refusing to resume an existing workspace');
  }
}
const source = process.env.BRIDGE_DEV_IMAGE_DIR;
if (!source || !(await stat(source)).isDirectory() || (await readdir(source)).length) throw Error('Expected empty development image source');
await mkdir(output, { recursive: true });
let socket;
const child = spawn(executable, [], { stdio: 'ignore', env: { ...process.env,
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-address=127.0.0.1 --remote-debugging-port=9222' } });
child.on('error', error => console.error(error));
async function until(work, label) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    const value = await work();
    if (value) return value;
    await delay(100);
  }
  throw Error(`Missing ${label}`);
}
try {
  const page = await until(async () => {
    if (child.exitCode !== null) throw Error('Native app exited');
    try {
      const pages = await (await fetch('http://127.0.0.1:9222/json/list')).json();
      return pages.find(item => item.type === 'page' && item.url.includes('tauri.localhost'));
    } catch { return undefined; }
  }, 'native WebView2 debug target');
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let sequence = 0;
  const pending = new Map();
  socket.onmessage = event => {
    const reply = JSON.parse(event.data);
    if (pending.has(reply.id)) {
      const { resolve, reject, timer } = pending.get(reply.id); pending.delete(reply.id); clearTimeout(timer);
      if (reply.error) reject(Error(JSON.stringify(reply.error))); else resolve(reply.result);
    }
  };
  function call(method, params = {}) {
    const id = ++sequence;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { pending.delete(id); reject(Error(`CDP timeout: ${method}`)); }, 10000);
      pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }));
    });
  }
  let contextId;
  async function evaluate(expression) {
    const result = await call('Runtime.evaluate', { expression, contextId, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  }
  await until(() => evaluate('!!window.__TAURI_INTERNALS__ && !!document.querySelector(".app-shell")'), 'embedded app');
  // Tauri's IPC properties are immutable in the main world. Use a debugger
  // isolated world with its own fixture IPC, sharing only the native WebView DOM.
  // Re-run the exact compiled entry; no product hooks or native setup mutations.
  const entry = await evaluate('document.querySelector("script[type=module][src]").src');
  const bundle = await readFile(join(fileURLToPath(new URL('../app/dist/assets/', import.meta.url)), new URL(entry).pathname.split('/').pop()), 'utf8');
  const { frameTree } = await call('Page.getFrameTree');
  const fixtures = [
    ['permission', { operation: 'start', cause: 'permission', codes: [5] }, 'does not have permission'],
    ['preflight', null, 'Virtualization is turned off or unavailable on this PC.'],
    ['prepare', { operation: 'prepare', cause: 'preparation', codes: [] }, 'existing cluster and agent files are kept'],
    ['download-reserved', { operation: 'download', cause: 'network', codes: [] }, 'Your agent files are kept'],
    ['unknown', { operation: 'start', cause: 'unknown', codes: [] }, "couldn't determine"],
    ['cli', 'modelCaptureUnavailable', 'gh auth login --hostname github.com --web'],
    ['sign-in', 'modelCaptureFailed', 'gh auth status --hostname github.com'],
    ['model-store', 'modelStore', 'could not save or load your sign-in'],
    ['model-copy', 'modelCaptureFailed', 'Share the visible error message'],
    ['cluster-empty', null, 'No kubectl contexts were found'],
    ['cluster-configuration', { check: 'configuration', result: 'unknown' }, 'kubeconfig could not be read'],
    ['cluster-permission', { check: 'providers', result: 'permissionDenied' }, 'Reading AI connections failed. Access was denied.'],
    ['cluster-unknown', { check: 'orkaIdentity', result: 'unknown' }, 'The cause could not be established'],
    ['cluster-reload-pending', null, 'Reload pending'],
    ['cluster-reloaded', null, 'corrected-context'],
  ];
  for (const theme of ['light', 'dark']) {
    await call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    for (const [name, failure, expected] of fixtures) {
      const cluster = name.startsWith('cluster-');
      const model = typeof failure === 'string';
      ({ executionContextId: contextId } = await call('Page.createIsolatedWorld', { frameId: frameTree.frame.id, worldName: `recovery-${theme}-${name}` }));
      await evaluate(`(() => {
        const failure = ${JSON.stringify(failure)};
        const context = { id: '00000000-0000-4000-8000-000000000001', name: 'fixture-context' };
        const target = { id: '00000000-0000-4000-8000-000000000002', context_id: context.id, context_name: context.name, service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'orka:fixture' };
        const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'fixture-model', ready: true };
        if (${JSON.stringify(name)} === 'model-copy') Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => { throw Error('Fixture clipboard unavailable'); } } });
        localStorage.setItem('bridge.welcomeSeen.v1', '1');
        window.isTauri = true;
        window.__TAURI_INTERNALS__ = { invoke: async command => {
          if (command === 'setup_status') return { selected: ${JSON.stringify(cluster ? null : 'localVm')}, localVm: ${JSON.stringify(cluster ? 'idle' : model ? 'ready' : 'failed')}, model: ${JSON.stringify(model ? 'available' : 'locked')}, ${name === 'preflight' ? "preflightFailure: 'virtualizationDisabled'," : model || cluster ? '' : 'workspaceFailure: failure,'} localVmError: null };
          if (command === 'setup_connect_model') throw { kind: failure };
          if (command === 'cluster_contexts') {
            if (${JSON.stringify(name)} === 'cluster-configuration') throw failure;
            return ${JSON.stringify(name)} === 'cluster-empty' ? [] : [context];
          }
          if (command === 'cluster_discover') return [target];
          if (command === 'cluster_use') return failure ? { status: 'unavailable', providers: [], failure } : { status: 'connected', providers: [provider], failure: null };
          if (command === 'cluster_reload') return ${JSON.stringify(name)} === 'cluster-reload-pending' ? new Promise(() => {}) : [{ ...context, id: '00000000-0000-4000-8000-000000000004', name: 'corrected-context' }];
          if (command === 'setup_requirements') return null;
          if (command === 'cluster_places') return { places: [], selectedKey: null };
          if (command === 'engine_call') throw { kind: 'unavailable' };
          throw Error('Unmocked fixture command: ' + command);
        } };
        document.getElementById('root').replaceWith(Object.assign(document.createElement('div'), { id: 'root' }));
      })()`);
      await evaluate(bundle);
      // VM-ready is unfinished until the AI is connected.
      if (model) await clickAction('Connect the AI');
      if (model) {
        await until(() => evaluate(`!![...document.querySelectorAll('button')].find(button => button.textContent === 'Connect the AI')`), 'model action');
        await evaluate(`[...document.querySelectorAll('button')].find(button => button.textContent === 'Connect the AI').click()`);
      }
      if (cluster) {
        // Add a cluster offers only an existing cluster: its context choice opens directly.
        await clickAction('Add a cluster');
        if (!['cluster-empty', 'cluster-configuration'].includes(name)) {
          await until(() => evaluate(`document.querySelector('#cluster-context')?.options.length > 1`), 'fixture contexts');
          await evaluate(`(() => { const select = document.querySelector('#cluster-context'); select.value = select.options[1].value; select.dispatchEvent(new Event('change', { bubbles: true })); })()`);
          await until(() => evaluate(`!![...document.querySelectorAll('button')].find(button => button.textContent === 'Use this cluster')`), 'cluster Use');
          await evaluate(`[...document.querySelectorAll('button')].find(button => button.textContent === 'Use this cluster').click()`);
          if (name.startsWith('cluster-reload')) {
            await until(() => evaluate(`document.body.textContent.includes('Access confirmed')`), 'confirmed fixture access');
            await evaluate(`[...document.querySelectorAll('button')].find(button => button.textContent === 'Reload kubeconfig').click()`);
          }
        }
      }
      if (name === 'model-copy') {
        await until(() => evaluate(`!![...document.querySelectorAll('button')].find(button => button.textContent === 'Copy details for help')`), 'model copy');
        await evaluate(`[...document.querySelectorAll('button')].find(button => button.textContent === 'Copy details for help').click()`);
      }
      // Local panels stay mounted but hidden under Places; require the recovery alert to be on screen.
      await until(() => evaluate(`document.body.textContent.includes(${JSON.stringify(expected)}) && ${cluster ? '!!document.querySelector(\'.cluster-card\')' : `[...document.querySelectorAll('[role=alert]')].some(item => item.getClientRects().length)`}`), `${name} recovery`);
      if (!model && !cluster) {
        const recovery = await evaluate(`({ progress: !!document.querySelector('.setup-progress'), rows: [...document.querySelectorAll('[aria-label="Setup steps"] li')].map(row => row.querySelector('b').textContent) })`);
        if (recovery.progress || recovery.rows.length !== 1 || name === 'preflight' && recovery.rows[0] !== 'Check this computer') throw Error(`${name}: empty progress card or waiting rows remain: ${JSON.stringify(recovery)}`);
      }
      if (cluster) await evaluate(`document.querySelector('.cluster-card').scrollIntoView({ block: 'end' })`);
      await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
      const image = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${name}.png`), Buffer.from(image.data, 'base64'));
      console.log(`Captured native WebView2 renderer fixture: ${theme}/${name}`);
    }
  }
  // Setup-progress states: renderer fixtures for transfer, startup, ready and a queued model choice.
  // Fixture IPC only: nothing here exercises real downloads, the VM or credentials.
  const GiB = 1024 ** 3;
  const transfer = { phase: 'materializing', bytes: Math.round(1.1 * GiB), totalBytes: Math.round(2.8 * GiB), elapsedMs: 20000, remainingMs: 30000 };
  const startup = elapsedMs => ({ phase: 'starting', bytes: null, totalBytes: null, elapsedMs, remainingMs: null });
  const setupFixtures = [
    ['setup-transfer', 'installing', transfer, false, 'Installing, 1.1 GiB of 2.8 GiB'],
    ['setup-transfer-queued', 'installing', transfer, true, 'Connects when the local cluster is ready.'],
    // A valid first observation, then every later read fails: the card must say unavailable, not keep the old bar.
    ['setup-unavailable', 'installing', transfer, false, 'Local cluster status is unavailable.', true],
    // A platform without native model support (reported by the snapshot) offers no early choice: a Waiting row, no action.
    ['setup-transfer-unsupported', 'installing', transfer, false, 'Installing, 1.1 GiB of 2.8 GiB', false, false],
    ['setup-startup', 'starting', startup(12000), false, 'Rough estimate, not a measurement'],
    ['setup-startup-slow', 'starting', startup(45000), false, 'Taking longer than the rough 30-second estimate'],
    ['setup-ready', 'ready', null, false, 'The local cluster is running on this computer.'],
  ];
  const pair = { agent: '---\nname: web-reader\ndescription: Answers from the web page named in the request\n---\nFetch before answering.', tests: 'The checksum check.' };
  async function captureSetup(theme, size, [name, localVm, progress, queue, expected, failLater, modelSupported = true]) {
    ({ executionContextId: contextId } = await call('Page.createIsolatedWorld', { frameId: frameTree.frame.id, worldName: `setup-${theme}-${size}-${name}` }));
    await evaluate(`(() => {
      const pair = ${JSON.stringify(pair)};
      let reads = 0;
      localStorage.setItem('bridge.welcomeSeen.v1', '1');
      window.isTauri = true;
      window.__TAURI_INTERNALS__ = { invoke: async command => {
        if (command === 'setup_status' && ${JSON.stringify(!!failLater)} && reads++ > 0) throw Error('Fixture status unreadable');
        if (command === 'setup_status') return { selected: 'localVm', localVm: ${JSON.stringify(localVm)}, model: ${JSON.stringify(localVm === 'ready' ? 'available' : 'locked')}, modelSupported: ${JSON.stringify(modelSupported)}, workspaceProgress: ${JSON.stringify(progress)}, localVmError: null };
        if (command === 'setup_example') return pair;
        if (command === 'setup_requirements') return null;
        if (command === 'cluster_places') return { places: [], selectedKey: null };
        if (command === 'engine_call') throw { kind: 'unavailable' };
        throw Error('Unmocked fixture command: ' + command);
      } };
      document.getElementById('root').replaceWith(Object.assign(document.createElement('div'), { id: 'root' }));
    })()`);
    await evaluate(bundle);
    if (localVm === 'ready') {
      await until(() => evaluate(`!!document.querySelector('[aria-label="This computer"] button')`), 'ready local row');
      if (!await evaluate(`document.querySelector('[aria-label="This computer"] button').textContent === 'Connect the AI' && document.querySelector('[aria-label="This computer"] .pill.status-needs-you') !== null`)) throw Error('A ready VM without connected AI must not advertise green Running or Manage');
      await clickAction('Connect the AI');
    }
    if (queue) {
      await until(() => evaluate(`!![...document.querySelectorAll('button')].find(button => button.textContent === 'Connect the AI')`), 'queueable model action');
      await evaluate(`[...document.querySelectorAll('button')].find(button => button.textContent === 'Connect the AI').click()`);
    }
    await until(() => evaluate(`document.body.textContent.includes(${JSON.stringify(expected)}) && [...document.querySelectorAll('.setup-progress')].some(item => item.getClientRects().length)`), `${name} setup progress`);
    const chrome = await evaluate(`document.querySelector('header').innerText + document.querySelector('footer').innerText`);
    if (/step \d+ of 5|setting up,? step/i.test(chrome)) throw Error(`${name}: chrome repeats a step counter: ${chrome}`);
    if (localVm === 'ready' && !await evaluate(`document.querySelector('.setup-progress .pill')?.textContent === 'Step 4 of 5' && !document.querySelector('[aria-label="This computer"] .pill.status-passed')`)) throw Error(`${name}: unfinished local setup must remain step 4`);
    await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
    if (!modelSupported && !await evaluate(`document.body.textContent.includes('Waiting') && !document.body.textContent.includes('Queued') && ![...document.querySelectorAll('button')].some(button => button.textContent === 'Connect the AI')`)) {
      throw new Error(`${name}: an unsupported platform must show Waiting with no model action`);
    }
    const shoot = async suffix => {
      const image = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${size}-${name}${suffix}.png`), Buffer.from(image.data, 'base64'));
      console.log(`Captured native WebView2 renderer fixture: ${theme}/${size}/${name}${suffix}`);
    };
    await shoot('');
    if (size === 'compact') { // The long step history sits below the model action; show it too.
      await evaluate(`document.querySelector('main').scrollTop = document.querySelector('main').scrollHeight`);
      await shoot('-end');
    }
  }
  // Setup actions and the first test use the compiled app and synthetic IPC only.
  const actionFixtures = ['places-empty', 'places-populated', 'local-preflight', 'local-preflight-missing', 'ai-idle', 'ai-connecting', 'ai-connected', 'local-completed', 'local-stopped', 'ai-removal-confirmation', 'ai-removal-retry', 'first-test-running', 'first-test-result', 'first-test-badges', 'switch-place', 'designer'];
  async function clickAction(label) {
    const action = `[...document.querySelectorAll('button')].find(button => button.textContent === ${JSON.stringify(label)} && !button.disabled && button.getClientRects().length)`;
    await until(() => evaluate(`!!${action}`), label);
    await evaluate(`${action}.click()`);
  }
  async function captureActions(theme, size, name) {
    const first = name.startsWith('first-test-');
    const initialFrame = (await call('Page.getFrameTree')).frameTree.frame;
    let frameId = initialFrame.id;
    if (first) {
      // Replacing a DOM root does not unmount React in older isolated worlds.
      // Destroy their polling effects before arming another shared one-shot preference.
      contextId = undefined;
      const previous = initialFrame.loaderId;
      await call('Page.reload');
      await until(async () => {
        const current = (await call('Page.getFrameTree')).frameTree.frame;
        if (current.loaderId === previous) return false;
        frameId = current.id;
        try { return await evaluate('document.readyState === "complete" && !!document.querySelector(".app-shell")'); }
        catch { return false; } // The old execution context can disappear during navigation.
      }, 'fresh page before automatic-test fixture');
    }
    ({ executionContextId: contextId } = await call('Page.createIsolatedWorld', { frameId, worldName: `actions-${theme}-${size}-${name}` }));
    const places = name.startsWith('places-') || name.startsWith('local-preflight');
    await evaluate(`(() => {
      const pair = ${JSON.stringify(pair)};
      const name = ${JSON.stringify(name)};
      const documentId = '9b2b93c4-375c-4815-9717-39852a96e411';
      let selected = ${JSON.stringify(!first && !name.startsWith('places-') && !name.startsWith('local-preflight'))};
      let connected = false;
      let submitted = false;
      window.__bridgeRemovals = 0;
      window.__bridgeStops = 0;
      window.__bridgeBadgeState = 'passed';
      window.__bridgeRuns = 0;
      const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
      const status = () => ({ selected: selected ? 'localVm' : null, localVm: selected ? 'ready' : window.__bridgeStops ? 'cancelled' : 'idle', model: selected ? 'available' : 'locked' });
      const report = { verdict: 'failed', cases: [{ id: 'page-checksum', question: 'What checksum does the page show?', phase: 'Succeeded', answer: 'No checksum was given.', completedTools: ['web_fetch'], assertions: [{ id: 'checksum', kind: 'contains', expected: 'fixture-checksum', verdict: 'failed' }], verdict: 'failed' }] };
      const snapshot = () => {
        const running = submitted && name === 'first-test-running';
        const complete = submitted && (name === 'first-test-result' || name === 'first-test-badges');
        const badge = window.__bridgeBadgeState;
        if (name === 'first-test-badges') {
          report.verdict = report.cases[0].verdict = report.cases[0].assertions[0].verdict = badge === 'failed' ? 'failed' : 'passed';
          if (submitted && badge === 'unknown') throw Error('Lost sample status');
        }
        return { documentId, ...(name === 'first-test-badges' ? { source: badge === 'stale' ? { ...source, agent: 'c'.repeat(64) } : source } : {}), busy: running ? { documentId, name: 'web-reader' } : null, state: running ? 'running' : complete ? 'complete' : 'idle', canRun: !running, modelConnected: connected, stage: running ? 'observe' : complete ? 'done' : null, model: connected ? 'fixture-model' : null, report: complete ? report : null, acknowledged: [], pending: null, error: null };
      };
      localStorage.setItem('bridge.welcomeSeen.v1', '1');
      localStorage.removeItem('bridge.firstTest.v1');
      window.isTauri = true;
      window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
        if (command === 'setup_status') return status();
        if (command === 'setup_select_local') { selected = true; return status(); }
        if (command === 'setup_change_local') { window.__bridgeStops++; selected = false; connected = false; return status(); }
        if (command === 'setup_remove_model') { window.__bridgeRemovals++; throw { kind: 'modelBusy' }; }
        // Known and missing size facts: fixture IPC, not the native catalog.
        if (command === 'setup_requirements') return name === 'local-preflight' ? ${JSON.stringify({ memoryBytes: 4 * GiB, imageBytes: Math.round(2.8 * GiB), stateDiskBytes: 20 * GiB })} : null;
        if (command === 'setup_preflight') return { platform: 'windows', virtualization: 'ready', memory: 'ready', installedMemoryBytes: ${16 * GiB}, requiredMemoryBytes: ${4 * GiB} };
        if (command === 'cluster_places') return name === 'places-populated' ? { selectedKey: null, places: [
          { key: 'fixture-a', context: 'fixture-cluster', service: 'control / orka', namespace: 'agents', stale: false, observedAt: Date.now() - 600000, agents: [] },
          { key: 'fixture-b', context: 'fixture-secondary', service: 'control / orka', namespace: 'agents', stale: true, observedAt: null, agents: [] }] }
          : { places: [], selectedKey: null };
        if (command === 'setup_example') return pair;
        if (command === 'setup_connect_model') {
          if (name === 'ai-connecting') return new Promise(() => {});
          connected = true; return { ready: true };
        }
        if (command === 'agent_open') return { ...pair, documentId, name: 'web-reader', ...(name === 'first-test-badges' ? { source } : {}) };
        if (command === 'agent_run') { if (args?.bundledExample !== true) throw Error('Fixture automatic test requires native source guard'); if (submitted) throw Error('Fixture automatic test replayed'); submitted = true; window.__bridgeRuns++; return snapshot(); }
        if (command === 'agent_run_status') return snapshot();
        if (command === 'send_history' || command === 'cluster_contexts') return [];
        throw Error('Unmocked action fixture command: ' + command);
      } };
      document.getElementById('root').replaceWith(Object.assign(document.createElement('div'), { id: 'root' }));
    })()`);
    await evaluate(bundle);
    // VM-ready offers Connect the AI, not Manage. This click only opens local detail.
    if (first || name.startsWith('local-preflight')) await clickAction('Set up');
    else if (!places) await clickAction('Connect the AI');
    if (first) await clickAction('Continue');
    if (first || ['ai-connecting', 'ai-connected', 'local-completed', 'local-stopped', 'ai-removal-confirmation', 'ai-removal-retry'].includes(name)) await clickAction('Connect the AI');
    if (name === 'local-stopped') {
      await clickAction('Stop the local cluster');
      await until(() => evaluate(`document.querySelector('h1')?.textContent === 'Set up this computer' && window.__bridgeStops === 1`), 'Stop returns to local preflight');
    }
    if (name.startsWith('ai-removal-')) {
      await clickAction('Remove sign-in');
      if (await evaluate('window.__bridgeRemovals') !== 0) throw Error('Opening confirmation removed sign-in');
      await clickAction('Cancel');
      if (await evaluate('window.__bridgeRemovals') !== 0) throw Error('Cancelling confirmation removed sign-in');
      await clickAction('Remove sign-in');
      if (name === 'ai-removal-retry') {
        await clickAction('Remove sign-in');
        await until(() => evaluate(`!!document.querySelector('button.button-primary') && [...document.querySelectorAll('button.button-primary')].some(button => button.textContent === 'Retry removal')`), 'primary removal retry');
        await clickAction('Retry removal');
        await until(() => evaluate(`window.__bridgeRemovals === 2 && !document.querySelector('[aria-label="Remove sign-in confirmation"]')`), 'direct cleanup retry without reconfirmation');
      }
    }
    if (name === 'switch-place') {
      await clickAction('Back to clusters');
      await clickAction('Add a cluster');
    }
    if (name === 'designer') await clickAction('Read its instructions');
    if (name === 'first-test-badges') {
      await until(() => evaluate(`!!document.querySelector('.designer-work')`), 'sample result before badge recovery');
      await evaluate(`document.querySelector('button[aria-label="Clusters"]').click()`);
      await until(() => evaluate(`!!document.querySelector('.setup-history details')`), 'completed step disclosure');
      await evaluate(`document.querySelector('.setup-history details').open = true`);
      // Primary action and the single failed case are the Local cluster setup card's, not the setup history's.
      for (const [state, label, primary] of [['passed', 'Its test passed', 'See what it answered'], ['failed', 'Its test failed', 'See what it answered'], ['stale', 'Needs running', 'Run it again'], ['unknown', 'Result unknown', 'Check again']]) {
        await evaluate(`window.__bridgeBadgeState = ${JSON.stringify(state)}`);
        await until(() => evaluate(`[...document.querySelectorAll('.setup-history li')].find(row => row.querySelector('b')?.textContent === 'Run your first agent')?.querySelector('.step-result')?.textContent === ${JSON.stringify(label)}`), `sample badge ${state}`);
        const card = await evaluate(`(() => {
          const card = document.querySelector("[aria-label='Local cluster setup']");
          const shown = [...card.querySelectorAll('button')].filter(button => button.getClientRects().length);
          return { text: card.innerText, primary: shown.filter(button => button.classList.contains('button-primary')).map(button => button.textContent), result: shown.some(button => button.textContent === 'See what it answered'), page: document.body.innerText };
        })()`);
        const fail = reason => { throw Error(`sample ${state}: ${reason}: ${JSON.stringify(card)}`); };
        if (card.primary.join('|') !== primary) fail(`primary action should be ${primary}`);
        if (/step 5 of 5/i.test(card.page)) fail('first-test state is shown as unfinished setup step 5');
        if (state === 'stale' && !card.result) fail('the stale sample has a readable report, so its result stays inspectable');
        if (card.text.includes('Its test failed') !== (state === 'failed')) fail('failure message belongs only to the failed test');
        for (const [suffix, target] of [['', '[aria-label="Setup steps"] li:last-child, [aria-label="Completed setup steps"] li:last-child'], ['-card', '[aria-label="Local cluster setup"]']]) {
          await evaluate(`document.querySelector(${JSON.stringify(target)}).scrollIntoView({ block: 'center' })`);
          await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
          const image = await call('Page.captureScreenshot', { format: 'png' });
          await writeFile(join(output, `${theme}-${size}-sample-${state}${suffix}.png`), Buffer.from(image.data, 'base64'));
        }
      }
      // Only the automatic test ran; no state offered here submitted another agent_run.
      if (await evaluate('window.__bridgeRuns') !== 1) throw Error('A sample state submitted a run other than the automatic test');
      return;
    }
    const expected = { 'places-empty': 'No other clusters added yet', 'places-populated': 'Not checked yet', 'local-preflight': 'Data disk, up to', 'local-preflight-missing': 'Data disk, up to', 'ai-idle': 'GitHub CLI must be installed and signed in', 'ai-connecting': 'Connecting', 'ai-connected': 'Connected', 'first-test-running': 'Run your first agentRunning', 'first-test-result': 'No checksum was given.', 'local-completed': 'Local cluster running', 'local-stopped': 'Set up this computer', 'ai-removal-confirmation': 'Disconnect the AI and remove Bridge’s saved sign-in?', 'ai-removal-retry': 'Press Retry removal', 'switch-place': 'Add a cluster', designer: 'Instructions and settings' };
    await until(() => evaluate(`document.body.textContent.includes(${JSON.stringify(expected[name])})`), `${name} action state`).catch(async error => {
      throw Error(`${error.message}: ${await evaluate('document.body.innerText')}`);
    });
    if (places) {
      // Count visible controls: hidden local panels stay mounted under Places.
      const probe = await evaluate(`(() => {
        const count = label => [...document.querySelectorAll('button')].filter(button => button.textContent.trim() === label && button.getClientRects().length).length;
        return { setUp: count('Set up'), connect: count('Connect'), add: count('Add a cluster'), back: count('Back to clusters'), continue: count('Continue'), radios: document.querySelectorAll('input[type=radio]').length, text: document.body.innerText };
      })()`);
      const fail = reason => { throw Error(`${name}: ${reason}: ${JSON.stringify(probe)}`); };
      if (probe.radios || /Where should your agents run|Engine diagnostics|Unavailable in this build|Reading…/.test(probe.text)) fail('removed chooser, diagnostics, caveat or unsettled size is visible');
      if (name === 'places-empty' && (probe.setUp !== 1 || probe.add !== 1 || probe.connect)) fail('empty Places needs only Set up and Add a cluster');
      if (name === 'places-populated' && (probe.setUp !== 1 || probe.connect !== 2 || !probe.text.includes('Last checked') || probe.text.includes('Selected for Designer'))) fail('remembered places need Last checked, Not checked yet and one Connect each');
      if (name.startsWith('local-preflight') && (!probe.back || !probe.continue || !['Memory', 'Cluster download', 'Data disk, up to'].every(label => probe.text.includes(label)))) fail('local detail needs Back to clusters, Continue and the three size rows');
      if (name === 'local-preflight' && !probe.text.includes('GiB')) fail('known sizes were not shown');
      if (name === 'local-preflight-missing' && !probe.text.includes('Not available yet')) fail('missing sizes need plain explanatory copy');
    }
    if (name === 'first-test-result' || name === 'designer') {
      await until(() => evaluate(`!!document.querySelector('.designer-work')`), 'Designer columns');
      const layout = await evaluate(`(() => {
        const tests = document.querySelector('.designer-tests').getBoundingClientRect();
        const editor = document.querySelector('.designer-file').getBoundingClientRect();
        const send = document.querySelector('.designer-checks').getBoundingClientRect();
        return { tests: { x: tests.x, y: tests.y, bottom: tests.bottom }, editor: { x: editor.x, y: editor.y, bottom: editor.bottom }, sendY: send.y, width: innerWidth, height: innerHeight, scrollWidth: document.querySelector('main').scrollWidth };
      })()`);
      if (size === 'wide' ? Math.abs(layout.tests.y - layout.editor.y) > 2 || layout.tests.x <= layout.editor.x : layout.tests.bottom > layout.editor.y + 2) throw Error(`Test visibility failed: ${JSON.stringify(layout)}`);
      if (layout.sendY < layout.editor.bottom || layout.scrollWidth > layout.width + 2) throw Error(`Editor/Send layout failed: ${JSON.stringify(layout)}`);
      await writeFile(join(output, `${theme}-${size}-${name}-layout.json`), JSON.stringify(layout, null, 2));
    }
    if (name.startsWith('ai-removal-')) await evaluate(`document.querySelector('.setup-model').scrollIntoView({ block: 'center' })`);
    await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
    const image = await call('Page.captureScreenshot', { format: 'png' });
    await writeFile(join(output, `${theme}-${size}-${name}.png`), Buffer.from(image.data, 'base64'));
    if (['ai-connected', 'local-completed'].includes(name)) {
      const complete = await evaluate(`({ heading: document.querySelector('h1').textContent, pill: document.querySelector('.setup-progress .pill')?.textContent, details: !!document.querySelector('.setup-history details:not([open])'), text: document.body.innerText, model: document.querySelector('.setup-model').innerText })`);
      if (complete.heading !== 'Local cluster' || complete.pill !== 'Local cluster running' || /step \d+ of 5/i.test(complete.text) || !complete.details || !complete.text.includes('Stop the local cluster') || complete.model.includes('GitHub CLI must be installed')) throw Error(`Completed local page is not collapsed: ${JSON.stringify(complete)}`);
    }
    if (name === 'ai-idle' && await evaluate(`document.querySelector('.setup-model').innerText.includes('Remove sign-in')`)) throw Error('Unused sign-in removal is visible before connecting');
    if (await evaluate(`document.body.innerText.includes('Change where agents run')`)) throw Error('Removed change panel is still visible');
    console.log(`Captured native WebView2 action fixture: ${theme}/${size}/${name}`);
  }
  const notes = [];
  for (const theme of ['light', 'dark']) {
    await call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    for (const fixture of setupFixtures) await captureSetup(theme, 'wide', fixture);
    for (const name of actionFixtures) await captureActions(theme, 'wide', name);
  }
  // Resize the real native window so its client area is 540x900, as a user would. Win32 SetWindowPos through
  // PowerShell, then verify what the embedded WebView actually reports; never assume the resize worked.
  const target = { width: 540, height: 900 };
  const resizeScript = `
$ErrorActionPreference = 'Stop'
Add-Type @'
using System; using System.Runtime.InteropServices;
public struct WinRect { public int L, T, R, B; }
public static class WinSize {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out WinRect r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out WinRect r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int h2, uint flags);
}
'@
$h = (Get-Process -Id ${child.pid}).MainWindowHandle
if ($h -eq [IntPtr]::Zero) { throw 'No native main window handle' }
for ($i = 0; $i -lt 4; $i++) {
  $w = New-Object WinRect; $c = New-Object WinRect
  [void][WinSize]::GetWindowRect($h, [ref]$w); [void][WinSize]::GetClientRect($h, [ref]$c)
  $dw = ${target.width} - ($c.R - $c.L); $dh = ${target.height} - ($c.B - $c.T)
  if ($dw -eq 0 -and $dh -eq 0) { break }
  [void][WinSize]::SetWindowPos($h, [IntPtr]::Zero, $w.L, $w.T, ($w.R - $w.L) + $dw, ($w.B - $w.T) + $dh, 0x0014)
  Start-Sleep -Milliseconds 300
}
$c = New-Object WinRect; [void][WinSize]::GetClientRect($h, [ref]$c)
"client=$($c.R - $c.L)x$($c.B - $c.T)"`;
  // EncodedCommand avoids any argument-quoting ambiguity for the embedded C#.
  const resized = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-EncodedCommand', Buffer.from(resizeScript, 'utf16le').toString('base64')], { encoding: 'utf8', timeout: 20000 });
  let mode = 'native-window-resize';
  const viewport = async () => evaluate('({ width: innerWidth, height: innerHeight, dpr: devicePixelRatio })');
  ({ executionContextId: contextId } = await call('Page.createIsolatedWorld', { frameId: frameTree.frame.id, worldName: 'viewport-probe' }));
  await delay(500);
  let reported = await viewport();
  const close = (a, b) => Math.abs(a - b) <= 2;
  if (resized.status !== 0 || !close(reported.width * reported.dpr, target.width) || !close(reported.height * reported.dpr, target.height)) {
    // Limitation, reported honestly: the window could not be resized, so use renderer viewport emulation.
    console.warn(`Native window resize unverified (exit ${resized.status}); using renderer viewport emulation. ${resized.stderr ?? ''}`);
    mode = 'renderer-viewport-emulation';
    await call('Emulation.setDeviceMetricsOverride', { width: target.width, height: target.height, deviceScaleFactor: 1, mobile: false });
    reported = await viewport();
  }
  notes.push(`Compact captures: ${mode}. Requested content area ${target.width}x${target.height}; WebView reported ${reported.width}x${reported.height} CSS px at devicePixelRatio ${reported.dpr}. ${resized.stdout.trim()}`);
  for (const theme of ['light', 'dark']) {
    await call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    for (const fixture of setupFixtures) await captureSetup(theme, 'compact', fixture);
    for (const name of actionFixtures) await captureActions(theme, 'compact', name);
  }
  await writeFile(join(output, 'compact-window.txt'), notes.join('\n') + '\n');
  await writeFile(join(output, 'README.txt'), 'Injected renderer states in the final-head embedded Windows app WebView2. Setup-progress states (setup-*), Places and local size states (places-*, local-preflight*), completed local/Stop pages (local-completed, local-stopped), removal confirmation/retry (ai-removal-*), Add-a-place stages (cluster-*) and setup actions/first-test/Designer states use synthetic IPC. Designer layout JSON records test/editor/Send geometry. compact-window.txt says whether 540x900 was a real native window resize or renderer viewport emulation. Not native IPC, VM, credential or download-path verification. Download is reserved and unreachable from current native startup. Themes are WebView2 media emulation; system preferences are unchanged.\n');
  // Keep the source identity next to images without inferring it from branch names.
  await writeFile(join(output, 'head.txt'), await readFile(join(output, '..', 'native-lifecycle', 'head.txt')));
} finally {
  socket?.close();
  await new Promise(resolve => {
    if (child.exitCode !== null || child.signalCode !== null || !child.pid) { resolve(); return; }
    child.once('exit', resolve);
    child.kill();
  });
}
