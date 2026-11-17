// Hosted-CI renderer fixtures in the real app's WebView2, not native IPC proofs.
// CDP is exposed only on loopback for this child process; no product test hooks.
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';

if (process.platform !== 'win32' || process.env.GITHUB_ACTIONS !== 'true') throw Error('Hosted Windows CI only');
const [executableArgument, outputArgument, captureScope = 'all'] = process.argv.slice(2);
if (!executableArgument || !outputArgument || process.argv.length > 5) throw Error('Expected executable, output directory and optional capture scope');
if (!['all', 'core', 'chat-agent-clusters'].includes(captureScope)) throw Error('Unknown capture scope');
const executable = resolve(executableArgument), output = resolve(outputArgument);
// Keep the added UI matrix out of the existing recovery step's fixed deadline.
const runCore = captureScope !== 'chat-agent-clusters';
const runUi = captureScope !== 'core';
for (const root of [process.env.APPDATA, process.env.LOCALAPPDATA]) {
  if (!root) throw Error('Missing native profile root');
  for (const leaf of ['setup-v1/local-vm', 'local-vm-v1']) {
    if (existsSync(join(root, 'ai.orka.bridge', leaf))) throw Error('Refusing to resume an existing workspace');
  }
}
const source = process.env.BRIDGE_DEV_IMAGE_DIR;
if (!source || !(await stat(source)).isDirectory() || (await readdir(source)).length) throw Error('Expected empty development image source');
await mkdir(output, { recursive: true });
const replyReason = 'Conversation replies need an authenticated gateway conversation, which local tests and Chat do not have.';
const replySavedAgent = '---\nname: web-reader\nmodel: fixture-model\nkmx:\n  runtime: orka\n  schemaVersion: 1\n  tools:\n    - kind: builtin\n      name: reply_in_conversation\n---\nFetch before answering.';
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
  const fixtures = runCore ? [
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
    ['cluster-permission', { check: 'providers', result: 'permissionDenied' }, "Couldn't read AI connections from this cluster. Access was denied."],
    ['cluster-unknown', { check: 'orkaIdentity', result: 'unknown' }, 'The reason is unknown.'],
    ['cluster-reload-pending', null, 'Reload pending'],
    ['cluster-reloaded', null, 'corrected-context'],
  ] : [];
  for (const theme of ['light', 'dark']) {
    await call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    for (const [name, failure, expected] of fixtures) {
      const cluster = name.startsWith('cluster-');
      const model = typeof failure === 'string';
      ({ executionContextId: contextId } = await call('Page.createIsolatedWorld', { frameId: frameTree.frame.id, worldName: `recovery-${theme}-${name}` }));
      await evaluate(`(() => {
        const failure = ${JSON.stringify(failure)};
        const context = { id: '00000000-0000-4000-8000-000000000001', name: 'fixture-context', current: true };
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
          if (command === 'cluster_places') return { places: [], selectedKey: null, persistenceState: 'complete' };
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
          await chooseContext('fixture-context');
          await until(() => evaluate(`!![...document.querySelectorAll('button')].find(button => button.textContent === 'Use this cluster')`), 'cluster Use');
          await evaluate(`[...document.querySelectorAll('button')].find(button => button.textContent === 'Use this cluster').click()`);
          if (name.startsWith('cluster-reload')) {
            await until(() => evaluate(`document.body.textContent.includes('Access confirmed')`), 'confirmed fixture access');
            await evaluate(`[...document.querySelectorAll('button')].find(button => button.textContent === 'Reload kubeconfig').click()`);
            if (name === 'cluster-reloaded') {
              await until(() => evaluate(`!!document.querySelector('button#cluster-context:not(:disabled)')`), 'reloaded context picker');
              await evaluate(`document.querySelector('button#cluster-context').click()`);
            }
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
        const recovery = await evaluate(`({ bar: !!document.querySelector('.setup-progress [role="progressbar"]'), rows: [...document.querySelectorAll('[aria-label="Setup steps"] > li')].map(row => row.querySelector('b').textContent), failed: !!document.querySelector('[aria-label="Setup steps"] > li:nth-child(2).step-failed .workspace-failure') })`);
        if (recovery.bar || recovery.rows.length !== 4 || !recovery.failed) throw Error(`${name}: recovery must stay inside workspace step 2: ${JSON.stringify(recovery)}`);
      }
      if (cluster) await evaluate(`document.querySelector('.cluster-card').scrollIntoView({ block: 'end' })`);
      await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
      const image = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${name}.png`), Buffer.from(image.data, 'base64'));
      console.log(`Captured native WebView2 renderer fixture: ${theme}/${name}`);
    }
  }
  async function checkLayout(theme, size, name) {
    await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
    const layout = await evaluate(`(() => {
      const shown = selector => [...document.querySelectorAll(selector)].filter(node => node.getClientRects().length);
      const rect = node => { if (!node) return null; const r = node.getBoundingClientRect(); return { x: r.x, y: r.y, right: r.right, bottom: r.bottom, width: r.width }; };
      const main = document.querySelector('main');
      return {
        width: innerWidth, scrollWidth: main.scrollWidth, clientWidth: main.clientWidth,
        tabs: shown('.page-tab').map(node => ({ ...rect(node), grow: getComputedStyle(node).flexGrow })),
        host: rect(document.querySelector('.host-chip')), hostLabel: rect(document.querySelector('.host-label')),
        subtitles: shown('.tab-copy small').filter(node => getComputedStyle(node).display !== 'none').length,
        quiet: shown('.button-quiet').map(node => ({ start: getComputedStyle(node).paddingInlineStart, end: getComputedStyle(node).paddingInlineEnd })),
        scrollbar: { width: getComputedStyle(main).scrollbarWidth, color: getComputedStyle(main).scrollbarColor },
        hostTone: document.querySelector('.host-chip .live-dot')?.className,
        overview: document.querySelector('#tab-status-Overview')?.textContent,
        forms: shown('.cluster-card > :not(.cluster-actions):not([role="group"]), .local-preflight > .card').map(node => ({ ...rect(node), limit: node.matches('.local-preflight > .card') ? 554 : 520 })),
        rows: shown('.place-row').map(row => ({ name: rect(row.querySelector('.place-summary')), pill: rect(row.querySelector(':scope > .pill')), action: rect(row.querySelector(':scope > button')), row: rect(row) })),
        checks: shown('.cluster-checks li').map(row => ({ row: rect(row), card: rect(row.closest('.cluster-card')) })),
        setup: (() => { const root = shown('.workspace-setup-grid')[0]; if (!root) return null; const get = selector => rect(root.querySelector(selector)); return { progress: get('.setup-progress'), model: get('.setup-model-slot:not(:empty)'), meet: get('.workspace-meet') }; })(),
        designer: (() => { const root = shown('.designer-work')[0]; if (!root) return null; const get = selector => rect([...root.querySelectorAll(selector)].find(node => node.getClientRects().length)); return { tests: get('.designer-tests:not([hidden])'), editor: get('.designer-file'), send: get('.send-panel:not([hidden])'), history: get('.send-history') }; })(),
        fallback: { file: rect(shown('.designer > .designer-file')[0]), history: rect(shown('.designer > .designer-checks .send-history')[0]) },
      };
    })()`);
    const fail = reason => { throw Error(`${name}/${size}: ${reason}: ${JSON.stringify(layout)}`); };
    if (layout.scrollWidth > layout.clientWidth + 2) fail('page overflows horizontally');
    if (layout.width > 820 && (layout.tabs.length !== 3 || layout.tabs.some(tab => Math.abs(tab.width - 180) > 1 || tab.grow !== '0') || layout.host.x - layout.tabs.at(-1).right < 16)) fail('wide tabs must stay 180px/non-growing with a 16px host gap');
    if (layout.width <= 820 && (layout.subtitles || layout.hostLabel.width > 160)) fail('compact subtitles/host label exceed their bounds');
    if (layout.quiet.some(item => item.start !== '0px' || item.end !== '0px')) fail('quiet control has inline padding');
    if (layout.scrollbar.width !== 'thin' || layout.scrollbar.color === 'auto') fail('page scrollbar must be thin and themed');
    if (['ai-reset-mismatch', 'ai-removal-retry'].includes(name) && !layout.hostTone?.split(' ').includes('ready')) fail('AI failure changed the ready workspace host tone');
    if (name === 'setup-failed' && !layout.hostTone?.split(' ').includes('bad')) fail('workspace failure lost its failed host tone');
    // Preflight card includes its 16px padding and 1px border on both sides.
    if (layout.forms.some(rect => rect.width > rect.limit + 2)) fail('form exceeds its content width');
    for (const { row, card } of layout.checks) {
      if (row.x < card.x + 16 || row.right > card.right - 16) fail('check border escapes card padding');
    }
    for (const { name: title, pill, action, row } of layout.rows) {
      if (layout.width <= 820 && (Math.abs(pill.y - action.y) > 2 || Math.abs(pill.x - title.x) > 2 || action.right > row.right - 16)) fail('compact pill/action row is not aligned below the name');
    }
    if (layout.rows.some(row => Math.abs(row.name.x - layout.rows[0].name.x) > 2)) fail('list names do not align');
    const setup = layout.setup;
    if (setup?.progress && setup.model) {
      if (layout.width <= 820) {
        if (setup.model.y < setup.progress.bottom - 2) fail('compact setup list must stay above the AI controls');
      } else {
        if (setup.meet && Math.abs(setup.meet.y - setup.model.bottom - 16) > 2) fail('side cards share the other column height');
      }
    }
    if (name === 'designer-loading' && (!layout.fallback.file || layout.fallback.history)) fail('loading must show only the editor skeleton');
    if (name === 'designer-open-error' && (!layout.fallback.file || !layout.fallback.history)) fail('failed-open fallback cards are not visible');
    if (layout.fallback.file && layout.fallback.history && Math.abs(layout.fallback.history.y - layout.fallback.file.bottom - 16) > 2) fail('fallback cards lose their spacing');
    const designer = layout.designer;
    if (designer?.editor && designer.tests) {
      if (layout.width > 820 ? Math.abs(designer.tests.y - designer.editor.y) > 2 || designer.tests.x <= designer.editor.x : designer.tests.y < designer.editor.bottom - 2) fail('editor/test column priority is wrong');
      if (designer.send && (designer.send.y < designer.tests.bottom - 2 || Math.abs(designer.send.x - designer.tests.x) > 2)) fail('Send is not below local tests in their shared stack');
      if (designer.send && designer.history && designer.history.y < designer.send.bottom - 2) fail('history precedes the current Send');
    }
    await writeFile(join(output, `${theme}-${size}-${name}-layout.json`), JSON.stringify(layout, null, 2));
  }
  // Setup-progress states: renderer fixtures for transfer, startup, ready and a queued model choice.
  // Fixture IPC only: nothing here exercises real downloads, the VM or credentials.
  const GiB = 1024 ** 3;
  const transfer = { phase: 'materializing', bytes: Math.round(1.1 * GiB), totalBytes: Math.round(2.8 * GiB), elapsedMs: 20000, remainingMs: 30000 };
  const startup = elapsedMs => ({ phase: 'starting', bytes: null, totalBytes: null, elapsedMs, remainingMs: null });
  const setupFixtures = runCore ? [
    ['setup-failed', 'failed', null, false, "couldn't determine"],
    ['setup-image-update', 'failed', null, false, 'This version of Bridge needs a new workspace.', false, true, { operation: 'verify', cause: 'workspaceUpdateRequired', codes: [] }],
    ['setup-image-unsupported', 'failed', null, false, 'This workspace uses different cluster files.', false, true, { operation: 'verify', cause: 'workspaceVersionUnsupported', codes: [] }],
    ['setup-transfer', 'installing', transfer, false, 'Installing, 1.1 GiB of 2.8 GiB'],
    ['setup-transfer-queued', 'installing', transfer, true, 'Connects when the workspace is ready.'],
    // A valid first observation, then every later read fails: the card must say unavailable, not keep the old bar.
    ['setup-unavailable', 'installing', transfer, false, 'Local cluster status is unavailable.', true],
    // A platform without native model support (reported by the snapshot) offers no early choice: a Waiting row, no action.
    ['setup-transfer-unsupported', 'installing', transfer, false, 'Installing, 1.1 GiB of 2.8 GiB', false, false],
    ['setup-startup', 'starting', startup(12000), false, '12 seconds elapsed'],
    ['setup-startup-slow', 'starting', startup(45000), false, '45 seconds elapsed'],
    ['setup-ready', 'ready', null, false, 'Your workspace is ready on this computer.'],
  ] : [];
  // The ready card title and frame references that setup and first-pass captures are compared with.
  const readyCardTitle = 'This computer';
  const fixtureAccount = 'synthetic-user';
  const pair = { agent: '---\nname: web-reader\ndescription: Answers from the web page named in the request\n---\nFetch before answering.', tests: '---\nschemaVersion: 1\n---\n```yaml agent-test\nid: page-checksum\ntitle: Copies the checksum from the page\ninput: What checksum does the page show?\nassertions: [{id: checksum, type: contains, value: fixture-checksum}]\n```\n' };
  // Meet derives its rows from the file, so setup captures use the sample's real tool declaration.
  const samplePair = { ...pair, agent: '---\nname: web-reader\ndescription: Answers from the web page named in the request\nmodel: gpt-5.6-terra\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools:\n    - kind: builtin\n      name: web_fetch\n---\nFetch before answering.' };
  async function captureSetup(theme, size, [name, localVm, progress, queue, expected, failLater, modelSupported = true, failure]) {
    ({ executionContextId: contextId } = await call('Page.createIsolatedWorld', { frameId: frameTree.frame.id, worldName: `setup-${theme}-${size}-${name}` }));
    await evaluate(`(() => {
      const pair = ${JSON.stringify(samplePair)};
      let reads = 0;
      localStorage.setItem('bridge.welcomeSeen.v1', '1');
      window.isTauri = true;
      window.__TAURI_INTERNALS__ = { invoke: async command => {
        if (command === 'setup_status' && ${JSON.stringify(!!failLater)} && reads++ > 0) throw Error('Fixture status unreadable');
        if (command === 'setup_status') return { selected: 'localVm', localVm: ${JSON.stringify(localVm)}, model: ${JSON.stringify(localVm === 'ready' ? 'available' : 'locked')}, modelSupported: ${JSON.stringify(modelSupported)}, workspaceProgress: ${JSON.stringify(progress)}, localVmError: null, ...${JSON.stringify(failure ? { workspaceFailure: failure } : {})} };
        if (command === 'setup_example') return pair;
        if (command === 'overview_files') return { state: 'complete', files: [], runActive: false, historyState: 'complete', runAgents: [] };
        if (command === 'setup_requirements') return null;
        if (command === 'cluster_places') return { places: [], selectedKey: null, persistenceState: 'complete' };
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
    const panel = localVm === 'failed' ? '.workspace-failure' : '.setup-progress';
    await until(() => evaluate(`document.body.textContent.includes(${JSON.stringify(expected)}) && [...document.querySelectorAll(${JSON.stringify(panel)})].some(item => item.getClientRects().length)`), `${name} setup progress`);
    if (!await evaluate(`document.querySelector('h1')?.textContent === 'This computer'`)) throw Error(`${name}: local page lost its stable title`);
    await until(() => evaluate(`document.querySelector('#tab-status-Overview')?.textContent === 'No saved agents yet'`), 'initial saved-only empty count');
    const chrome = await evaluate(`document.querySelector('header').innerText + document.querySelector('footer').innerText`);
    const observed = !failLater && localVm !== 'failed';
    const step = localVm === 'ready' ? 3 : 2;
    if (/step \d+ of 5/i.test(chrome) || observed && !chrome.includes(`step ${step} of 4`) || observed && size === 'wide' && !chrome.includes(`Setting up, step ${step} of 4`) || !observed && /step \d+ of 4/i.test(chrome)) throw Error(`${name}: incorrect shared step counter: ${chrome}`);
    const rows = await evaluate(`({ count: document.querySelectorAll('[aria-label="Setup steps"] > li').length, current: document.querySelector('[aria-current="step"] b')?.textContent, barRow: document.querySelector('[role="progressbar"]')?.closest('li')?.querySelector('b')?.textContent, value: document.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow') })`);
    if (rows.count !== 4 || observed && rows.current !== (step === 2 ? 'Get your workspace ready' : 'Choose the AI your agent uses')) throw Error(`${name}: list and shell disagree: ${JSON.stringify(rows)}`);
    if (observed && localVm !== 'ready' && rows.barRow !== 'Get your workspace ready') throw Error(`${name}: progress is outside the active workspace row`);
    if (localVm === 'starting' && rows.value !== null) throw Error(`${name}: startup must be indeterminate`);
    await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
    if (!modelSupported && !await evaluate(`document.body.textContent.includes('Waiting') && !document.body.textContent.includes('Queued') && ![...document.querySelectorAll('button')].some(button => button.textContent === 'Connect the AI')`)) {
      throw new Error(`${name}: an unsupported platform must show Waiting with no model action`);
    }
    const shoot = async suffix => {
      const image = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${size}-${name}${suffix}.png`), Buffer.from(image.data, 'base64'));
      console.log(`Captured native WebView2 renderer fixture: ${theme}/${size}/${name}${suffix}`);
      return image.data;
    };
    if (name.startsWith('setup-image-')) {
      const refusal = await evaluate(`(() => { const alert = [...document.querySelectorAll('.workspace-failure')].find(item => item.getClientRects().length); const retry = [...alert.querySelectorAll('button')].find(button => button.textContent === 'Try again'); return { text: alert.innerText, disabled: retry?.disabled, title: retry?.title, replace: [...document.querySelectorAll('button')].some(button => button.textContent === 'Set up new workspace') }; })()`);
      if (!refusal.disabled || refusal.title !== 'Open the matching Bridge version.' || !refusal.text.includes('Open the matching Bridge version.') || refusal.text.includes("couldn't be verified") || refusal.replace) throw Error(`${name}: misleading or actionable image refusal ${JSON.stringify(refusal)}`);
    }
    if (name === 'setup-transfer') {
      const meet = await evaluate(`(() => { const card = document.querySelector('.workspace-meet'); return { tools: [...card.querySelectorAll('.workspace-tool code')].map(code => code.textContent), text: card.innerText }; })()`);
      if (meet.tools.join('|') !== 'web_fetch' || !meet.text.includes('Copies the checksum from the page')) throw Error(`${name}: Meet must list the file's tools and tests: ${JSON.stringify(meet)}`);
    }
    if (queue) {
      const queued = await evaluate(`(() => { const status = [...document.querySelectorAll('.setup-model [role=status]')].find(item => item.textContent.includes('Queued')); const cancel = document.querySelector('button[aria-label="Cancel queued connection"]'); return { status: !!status, inChoice: !!status?.closest('fieldset'), cancel: cancel?.textContent, secondary: !!cancel?.classList.contains('button-secondary'), text: document.querySelector('.setup-model').innerText }; })()`);
      if (!queued.status || queued.inChoice || queued.cancel !== 'Cancel' || !queued.secondary || /local cluster/.test(queued.text)) throw Error(`${name}: queued AI must be a status row with a secondary Cancel: ${JSON.stringify(queued)}`);
    }
    await checkLayout(theme, size, name);
    const image = await call('Page.captureScreenshot', { format: 'png' });
    await writeFile(join(output, `${theme}-${size}-${name}.png`), Buffer.from(image.data, 'base64'));
    console.log(`Captured native WebView2 renderer fixture: ${theme}/${size}/${name}`);
    if (name === 'setup-transfer') await compareReference(image.data, theme, size, name, 'meet');
    if (name === 'setup-transfer-queued') await compareReference(image.data, theme, size, name, 'ai');
    if (localVm === 'failed') {
      await evaluate(`document.querySelector('.workspace-failure summary').click()`);
      await until(() => evaluate(`!!document.querySelector('.workspace-failure summary[aria-expanded="true"]')`), 'expanded recovery details');
      await shoot('-details');
    }
    if (size === 'compact') { // Also show controls below the fixed step list.
      await evaluate(`document.querySelector('main').scrollTop = document.querySelector('main').scrollHeight`);
      const end = await shoot('-end');
      if (name === 'setup-transfer' || name === 'setup-transfer-queued') {
        await compareReference(end, theme, size, `${name}-end`, name === 'setup-transfer' ? 'meet' : 'ai');
      }
    }
  }
  // Setup actions and the first test use the compiled app and synthetic IPC only.
  // first-test-passed, -unknown and -lost-reply each own one fresh automatic attempt after a page reload.
  const actionFixtures = runCore ? ['welcome', 'places-empty', 'places-populated', 'local-preflight', 'local-preflight-missing', 'local-preflight-unknown', 'local-preflight-blocked', 'local-preflight-memory', 'local-preflight-mixed', 'local-preflight-cluster', 'ai-idle', 'ai-key', 'ai-cli-missing', 'ai-connecting', 'ai-connected', 'ai-reset', 'local-completed', 'local-stopped', 'local-stop-failed', 'local-stop-lost-reply', 'ai-removal-confirmation', 'ai-removal-retry', 'first-test-running', 'first-test-result', 'first-test-badges', 'first-test-passed', 'first-test-unknown', 'first-test-lost-reply', 'switch-place', 'designer', 'designer-loading', 'designer-open-error', 'designer-connected', 'designer-catalog-busy', 'designer-file', 'designer-reply-saved', 'designer-failed', 'designer-passed', 'designer-stale', 'designer-running', 'designer-remote', 'local-cancel-running', 'local-cancel-cancelling', 'local-cancel-cancelled', 'local-cancel-unconfirmed', 'local-cancel-before-start'] : [];
  async function clickAction(label) {
    const action = `[...document.querySelectorAll('button')].find(button => button.textContent === ${JSON.stringify(label)} && !button.disabled && button.getClientRects().length)`;
    await until(() => evaluate(`!!${action}`), label);
    await evaluate(`${action}.click()`);
  }
  async function chooseContext(name) {
    await until(() => evaluate(`!!document.querySelector('button#cluster-context:not(:disabled)')`), 'context picker');
    await evaluate(`document.querySelector('button#cluster-context').click()`);
    const choice = `[...document.querySelectorAll('[role="dialog"][aria-label="Choose a context"] button')].find(button => button.textContent.includes(${JSON.stringify(name)}))`;
    await until(() => evaluate(`!!${choice}`), name);
    await evaluate(`${choice}.click()`);
  }
  // Designer is a same-design-system comparison for Chat and blank creation, not a dedicated target frame.
  // Designer, workspace, setup choices and AI recovery frames have both themes; cluster cards and setup frames 1c, 1d and 1e exist only in light.
  const referenceImage = async file => (await readFile(new URL(`../docs/images/${file}`, import.meta.url))).toString('base64');
  const references = {
    workspace: { light: await referenceImage('workspace-reference-light.png'), dark: await referenceImage('workspace-reference-dark.png'), label: 'Approved workspace frame (wide reference)' },
    designer: { light: await referenceImage('designer-reference-light.png'), dark: await referenceImage('designer-reference-dark.png'), label: 'Approved Designer frame (wide reference)' },
    clusters: { light: await referenceImage('cluster-reference-light.png'), label: 'Approved cluster cards (wide reference)' },
    'setup-choices': { light: await referenceImage('setup-choices-reference-light.png'), dark: await referenceImage('setup-choices-reference-dark.png'), label: 'Approved setup choices (wide reference)' },
    meet: { light: await referenceImage('setup-reference-meet.png'), label: 'Approved frame 1c, light (wide reference)' },
    ai: { light: await referenceImage('setup-reference-ai.png'), label: 'Approved frame 1d, light (wide reference, cropped)' },
    'first-pass': { light: await referenceImage('setup-reference-first-pass.png'), label: 'Approved frame 1e, light (wide reference)' },
    failed: { light: await referenceImage('ai-connection-failed-reference-light.png'), dark: await referenceImage('ai-connection-failed-reference-dark.png'), label: 'Approved AI connection failure frame (wide reference)' },
    reset: { light: await referenceImage('ai-reset-failed-reference-light.png'), dark: await referenceImage('ai-reset-failed-reference-dark.png'), label: 'Approved AI reset frame (wide reference)' },
  };
  const compareDesigner = (data, theme, size, name, workspace = false) => compareReference(data, theme, size, name, workspace ? 'workspace' : 'designer');
  async function compareReference(data, theme, size, name, kind) {
    const frame = references[kind];
    const comparison = await evaluate(`(async () => {
      const decode = async text => createImageBitmap(new Blob([Uint8Array.from(atob(text), c => c.charCodeAt(0))], { type: 'image/png' }));
      const actual = await decode(${JSON.stringify(data)}), reference = await decode(${JSON.stringify(frame[theme] ?? frame.light)});
      const width = actual.width, referenceHeight = Math.round(reference.height * width / reference.width);
      const canvas = document.createElement('canvas');
      canvas.width = width * 2 + 48; canvas.height = Math.max(actual.height, referenceHeight) + 58;
      const ctx = canvas.getContext('2d');
      ctx.fillStyle = ${JSON.stringify(theme === 'dark' ? '#1f2226' : '#f2f1ed')}; ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(actual, 16, 42); ctx.drawImage(reference, width + 32, 42, width, referenceHeight);
      ctx.fillStyle = ${JSON.stringify(theme === 'dark' ? '#e6e4df' : '#1b2026')}; ctx.font = '14px sans-serif';
      ctx.fillText('Native Bridge, synthetic test data', 16, 26);
      ctx.fillText(${JSON.stringify(frame.label)}, width + 32, 26);
      actual.close(); reference.close();
      return canvas.toDataURL('image/png').split(',')[1];
    })()`);
    await writeFile(join(output, `${theme}-${size}-${name}-comparison.png`), Buffer.from(comparison, 'base64'));
  }
  if (runUi) actionFixtures.push('chat-empty', 'chat-pending', 'chat-running', 'chat-answer', 'chat-session-null', 'chat-session-answer', 'new-agent-form', 'new-agent-created', 'cluster-picker-open', 'cluster-checking', 'cluster-failure');
  async function captureWorkspace(theme, size, name) {
    const card = '.returning-workspace';
    const capture = async (state, word, action, disabled = false) => {
      await until(() => evaluate(`document.querySelector('${card} .pill')?.textContent === ${JSON.stringify(word)}`), state);
      const probe = await evaluate(`(() => {
        const card = document.querySelector('${card}');
        const button = [...card.querySelectorAll('button')].find(b => b.textContent === ${JSON.stringify(action)});
        return { heading: document.querySelector('h1').textContent, text: card.innerText, disabled: button?.disabled,
          firstTest: localStorage.getItem('bridge.firstTest.v1'), bar: !!card.querySelector('[role=progressbar]'),
          choices: !!document.querySelector('.local-preflight input[type=radio]'), runs: window.__bridgeRuns,
          start: [...card.querySelectorAll('button')].some(b => b.textContent === 'Start'), width: innerWidth, height: innerHeight };
      })()`);
      if (probe.heading !== 'This computer' || probe.bar || probe.choices || probe.runs || probe.firstTest === 'pending' ||
          (action ? probe.disabled !== disabled : probe.start)) throw Error(state + ': invalid returning workspace: ' + JSON.stringify(probe));
      if (size === 'compact' && (probe.width !== 540 || probe.height !== 640)) throw Error('Workspace minimum viewport is not 540x640');
      if (state === 'starting' && !probe.text.includes('0:18 so far')) throw Error('Missing native-observed elapsed time');
      if (state === 'closed' && !probe.text.includes('Quit Bridge and reopen it')) throw Error('Closed workspace needs reopen remedy');
      if (state === 'stop-lost-reply' && !probe.text.includes('Workspace status is unavailable. Check again before starting it.')) throw Error('Lost Stop reply needs generic status-unavailable guidance');
      await evaluate(`document.querySelector('main').scrollTop = 0`);
      await checkLayout(theme, size, 'local-workspace-' + state);
      const image = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${size}-local-workspace-${state}.png`), Buffer.from(image.data, 'base64'));
      if (state === 'stopped') await writeFile(join(output, `${theme}-${size}-local-stopped.png`), Buffer.from(image.data, 'base64'));
      await compareDesigner(image.data, theme, size, 'local-workspace-' + state, true);
    };
    await clickAction('Stop the local cluster');
    await until(() => evaluate(`!!document.querySelector('[aria-label="Stop the workspace?"]')`), 'inline Stop confirmation');
    if (await evaluate('window.__bridgeStops') !== 0) throw Error('Opening Stop confirmation stopped the workspace');
    const confirmation = await call('Page.captureScreenshot', { format: 'png' });
    await writeFile(join(output, `${theme}-${size}-local-workspace-confirmation.png`), Buffer.from(confirmation.data, 'base64'));
    await compareDesigner(confirmation.data, theme, size, 'local-workspace-confirmation', true);
    await clickAction('Keep running');
    if (await evaluate('window.__bridgeStops') !== 0) throw Error('Keep running stopped the workspace');
    await clickAction('Stop the local cluster'); await clickAction('Stop workspace');
    await capture('stopping', 'Stopping', 'Start', true);
    if (name === 'local-stop-failed') {
      await evaluate(`window.__bridgeSettleWorkspace('stop-failed')`);
      await capture('stop-failed', "Couldn't stop", 'Try again', true);
      if (await evaluate('window.__bridgeStops') !== 1 || await evaluate('window.__bridgeLocalSelections') !== 0) throw Error('Stop failure must not restart or reselect');
      return;
    }
    if (name === 'local-stop-lost-reply') {
      await evaluate(`window.__bridgeSettleWorkspace('stop-lost-reply')`);
      await capture('stop-lost-reply', 'Status unavailable', 'Stop', true);
      await evaluate(`window.__bridgeWorkspaceUnreadable = false`);
      await capture('stop-reply-recovered', 'Stopped', 'Start');
      if (await evaluate('window.__bridgeStops') !== 1 || await evaluate('window.__bridgeLocalSelections') !== 0) throw Error('Lost Stop reply must recover by observation alone');
      return;
    }
    await evaluate(`window.__bridgeSettleWorkspace('cancelled')`);
    await capture('stopped', 'Stopped', 'Start');
    await clickAction('Start');
    await capture('starting', 'Starting', 'Cancel');
    await clickAction('Cancel');
    await until(() => evaluate(`document.querySelector('${card} .pill')?.textContent === 'Stopping'`), 'restart cancellation');
    await evaluate(`window.__bridgeSettleWorkspace('cancelled')`);
    await capture('cancelled', 'Stopped', 'Start');
    await clickAction('Start'); await evaluate(`window.__bridgeSettleWorkspace('failed')`);
    await capture('failed', "Couldn't start", 'Try again');
    await clickAction('Try again'); await evaluate(`window.__bridgeSettleWorkspace('ready')`);
    await capture('running', 'Running', 'Stop');
    await clickAction('Stop'); await clickAction('Stop workspace');
    await evaluate(`window.__bridgeSettleWorkspace('cancelled')`);
    await capture('stopped-again', 'Stopped', 'Start');
    await evaluate(`window.__bridgeWorkspaceUnreadable = true; window.dispatchEvent(new Event('focus'))`);
    await capture('unavailable', 'Status unavailable', 'Start', true);
    await evaluate(`window.__bridgeWorkspaceUnreadable = false; window.dispatchEvent(new Event('focus'))`);
    await capture('readable-again', 'Stopped', 'Start');
    await clickAction('Start'); await evaluate(`window.__bridgeSettleWorkspace('ready')`);
    await capture('running-again', 'Running', 'Stop');
    await clickAction('Stop'); await clickAction('Stop workspace');
    await evaluate(`window.__bridgeSettleWorkspace('closed')`);
    await capture('closed', 'Stopped', null);
    if (await evaluate('window.__bridgeStops') !== 4 || await evaluate('window.__bridgeLocalSelections') !== 4) throw Error('Workspace action sequence changed');
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
    const preflight = name.startsWith('local-preflight');
    const clusterScreen = name.startsWith('cluster-');
    const places = name === 'welcome' || name.startsWith('places-') || preflight || clusterScreen;
    await evaluate(`(() => {
      const pair = ${JSON.stringify(pair)};
      const name = ${JSON.stringify(name)};
      let documentId = '9b2b93c4-375c-4815-9717-39852a96e411';
      let selected = ${JSON.stringify(!first && !places)};
      const designer = name.startsWith('designer') || name.startsWith('chat-') || name.startsWith('new-agent-');
      let documentName = 'web-reader';
      if (designer) pair.agent = ${JSON.stringify('---\nname: web-reader\nmodel: ' + (name === 'designer-connected' ? 'gpt-5.6-terra' : 'fixture-model') + '\nkmx:\n  runtime: orka\n  schemaVersion: 1\n  tools: []\n---\nFetch before answering.')};
      if (name === 'designer-reply-saved') pair.agent = ${JSON.stringify(replySavedAgent)};
      let connected = designer && name !== 'designer';
      let submitted = false;
      let attemptId = null;
      const context = { id: '00000000-0000-4000-8000-000000000001', name: 'team-dev', current: true };
      const target = { id: '00000000-0000-4000-8000-000000000002', context_id: context.id, context_name: context.name, service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'fixture-image' };
      const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'team-ai', ready: true };
      window.__bridgeCatalogCalls = [];
      window.__bridgeOverviewLiveReads = 0;
      const localCancel = name.startsWith('local-cancel-');
      window.__bridgeCancellation = name.slice('local-cancel-'.length);
      window.__bridgeCancelCalls = [];
      const finished = { id: 'finished-answer', question: 'What did the page say?', phase: 'Succeeded', answer: 'The page says fixture-checksum.', completedTools: ['web_fetch'], assertions: [{ id: 'checksum', kind: 'contains', expected: 'fixture-checksum', verdict: 'passed' }], verdict: 'passed' };
      const cancelled = { id: 'unfinished-test', question: 'What does the next page say?', phase: 'Cancelled', answer: null, completedTools: [], assertions: [], verdict: 'cancelled' };
      window.__bridgeRemovals = 0;
      // Synthetic earlier-installation recovery IDs; the reset outcome is chosen by the driver.
      const recovery = { installation: '10000000-0000-4000-8000-000000000001', savedInstallation: '20000000-0000-4000-8000-000000000002', credentialSlot: '30000000-0000-4000-8000-000000000003' };
      window.__bridgeMismatch = name === 'ai-reset';
      window.__bridgeResets = [];
      window.__bridgeResetOutcome = 'credential';
      window.__bridgeCopied = [];
      if (name === 'ai-reset') Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => { window.__bridgeCopied.push(text); } } });
      window.__bridgeStops = 0;
      window.__bridgeLocalSelections = 0;
      window.__bridgeClusterUses = 0;
      // The badge walk needs Designer's automatic failure path; a passed submission would stay on its first-pass story.
      window.__bridgeBadgeState = name === 'first-test-badges' ? 'failed' : 'passed';
      window.__bridgeRuns = 0;
      const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
      const workspace = ['local-stopped', 'local-stop-failed', 'local-stop-lost-reply'].includes(name);
      let workspacePhase = 'ready';
      let stopFailed = false;
      let finishStop, rejectStop;
      window.__bridgeWorkspaceUnreadable = false;
      window.__bridgeSettleWorkspace = next => {
        if (workspacePhase === 'closing' && ['cancelled', 'closed'].includes(next)) {
          workspacePhase = next; finishStop(status()); finishStop = rejectStop = undefined;
        } else if (workspacePhase === 'closing' && ['stop-failed', 'stop-lost-reply'].includes(next)) {
          stopFailed = next === 'stop-failed';
          workspacePhase = stopFailed ? 'failed' : 'cancelled';
          window.__bridgeWorkspaceUnreadable = !stopFailed;
          rejectStop(Error(stopFailed ? 'Synthetic Stop failed' : 'Synthetic Stop reply lost'));
          finishStop = rejectStop = undefined;
        } else if (workspacePhase === 'starting' && ['failed', 'ready'].includes(next)) workspacePhase = next;
        else throw Error('Invalid synthetic workspace transition: ' + workspacePhase + ' -> ' + next);
      };
      const status = () => {
        if (window.__bridgeWorkspaceUnreadable) throw Error('Synthetic workspace status unreadable');
        const localVm = selected ? workspacePhase : window.__bridgeStops ? 'cancelled' : 'idle';
        return { selected: selected ? 'localVm' : null, localVm, model: localVm === 'ready' ? 'available' : 'locked', modelSupported: true,
          workspaceProgress: localVm === 'starting' ? { phase: 'starting', bytes: null, totalBytes: null, elapsedMs: 18000, remainingMs: null } : null,
          ...(localVm === 'failed' ? { workspaceFailure: stopFailed
            ? { operation: 'stop', cause: 'stopUnconfirmed', codes: [] }
            : { operation: 'start', cause: 'permission', codes: [5] } } : {}) };
      };
      const report = { verdict: 'failed', cases: [{ id: 'page-checksum', title: 'Copies the checksum from the page', question: 'What checksum does the page show?', phase: 'Succeeded', answer: 'No checksum was given.', completedTools: ['web_fetch'], assertions: [{ id: 'checksum', kind: 'contains', expected: 'fixture-checksum', verdict: 'failed' }], verdict: 'failed' }] };
      const snapshot = () => {
        if (localCancel) {
          const state = window.__bridgeCancellation;
          const running = state === 'running' || state === 'cancelling';
          const cancellation = state === 'running' ? 'none' : state === 'before-start' ? 'beforeStart' : state;
          return { documentId, source, busy: running ? { documentId, name: 'web-reader' } : null, state: running ? 'running' : 'complete', stage: running ? state === 'running' ? 'observe' : 'cancel' : 'done', canRun: !running, modelConnected: true, model: 'fixture-model', cancellation, canCancel: state === 'running', canRetryCancellation: state === 'unconfirmed', canCheckCancellation: state === 'unconfirmed', report: { verdict: state === 'unconfirmed' || state === 'running' || state === 'cancelling' ? 'unknown' : 'cancelled', cases: state === 'before-start' ? [] : state === 'cancelled' ? [cancelled, finished] : [finished] }, acknowledged: [], pending: null, error: null };
        }
        const running = name === 'designer-running' || submitted && name === 'first-test-running';
        const outcome = ['first-test-passed', 'first-test-unknown', 'first-test-lost-reply'].includes(name);
        const complete = ['designer-failed', 'designer-passed', 'designer-stale'].includes(name) || submitted && (name === 'first-test-result' || name === 'first-test-badges' || outcome);
        if (name === 'designer-passed' || name === 'first-test-passed' || name === 'first-test-lost-reply') { report.verdict = report.cases[0].verdict = report.cases[0].assertions[0].verdict = 'passed'; report.cases[0].answer = 'fixture-checksum'; }
        if (name === 'first-test-unknown') { report.verdict = report.cases[0].verdict = report.cases[0].assertions[0].verdict = 'unknown'; report.cases[0].phase = 'Unknown'; report.cases[0].answer = null; }
        const badge = window.__bridgeBadgeState;
        if (name === 'first-test-badges') {
          report.verdict = report.cases[0].verdict = report.cases[0].assertions[0].verdict = badge === 'failed' ? 'failed' : 'passed';
          if (submitted && badge === 'unknown') throw Error('Lost sample status');
        }
        return { documentId, ...(attemptId ? { attemptId } : {}), canCancel: running, ...(name === 'designer-stale' ? { source: { ...source, tests: 'c'.repeat(64) } } : {}), ...(name === 'first-test-badges' ? { source: badge === 'stale' ? { ...source, agent: 'c'.repeat(64) } : source } : {}), ...(outcome ? { source } : {}), busy: running ? { documentId, name: 'web-reader' } : null, state: running ? 'running' : complete ? 'complete' : 'idle', canRun: !running, modelConnected: connected, stage: running ? 'observe' : complete ? 'done' : null, model: connected ? 'fixture-model' : null, report: complete ? report : null, acknowledged: [], pending: null, error: null };
      };
      if (name === 'welcome') localStorage.removeItem('bridge.welcomeSeen.v1');
      else localStorage.setItem('bridge.welcomeSeen.v1', '1');
      localStorage.removeItem('bridge.firstTest.v1');
      window.isTauri = true;
      window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
        if (command === 'setup_status') return status();
        if (command === 'setup_select_local') { window.__bridgeLocalSelections++; selected = true; stopFailed = false; workspacePhase = workspace ? 'starting' : 'ready'; return status(); }
        if (command === 'setup_cancel_local') {
          window.__bridgeStops++; connected = false;
          if (!selected) throw Error('Stop must retain the selected workspace');
          workspacePhase = workspace ? 'closing' : 'cancelled';
          return workspace ? new Promise((resolve, reject) => { finishStop = resolve; rejectStop = reject; }) : status();
        }
        if (command === 'setup_change_local') {
          if (workspace || localCancel) throw Error('Stop must use setup_cancel_local');
          window.__bridgeStops++; selected = false; connected = false; return status();
        }
        if (command === 'agents_list') return [{ id: documentId, name: documentName, path: null, available: true }];
        if (command === 'agent_creation_root') return 'C:\\\\Users\\\\you\\\\Orka\\\\agents';
        if (command === 'setup_remove_model') { window.__bridgeRemovals++; throw { kind: 'modelBusy' }; }
        // Known and missing size facts: fixture IPC, not the native catalog.
        if (command === 'setup_requirements') return name.startsWith('local-preflight') && name !== 'local-preflight-missing' ? ${JSON.stringify({ memoryBytes: 4 * GiB, imageBytes: Math.round(2.8 * GiB), stateDiskBytes: 20 * GiB })} : null;
        if (command === 'setup_preflight') return { platform: 'windows', virtualization: ['local-preflight-unknown', 'local-preflight-mixed'].includes(name) ? 'unknown' : ['local-preflight-blocked', 'local-preflight-cluster'].includes(name) ? 'disabled' : 'ready', memory: name === 'local-preflight-unknown' ? 'unknown' : ['local-preflight-memory', 'local-preflight-mixed'].includes(name) ? 'insufficient' : 'ready', installedMemoryBytes: name === 'local-preflight-unknown' ? null : ['local-preflight-memory', 'local-preflight-mixed'].includes(name) ? ${2 * GiB} : ${16 * GiB}, requiredMemoryBytes: ${4 * GiB} };
        if (command === 'cluster_places') return name === 'places-populated' ? { selectedKey: null, persistenceState: 'complete', places: [
          { key: 'fixture-a', context: 'fixture-cluster', service: 'control / orka', namespace: 'agents', stale: false, observedAt: Date.now() - 600000, agents: [] },
          { key: 'fixture-b', context: 'fixture-secondary-with-a-long-cluster-name-to-wrap-without-clipping', service: 'control / orka', namespace: 'agents', stale: true, observedAt: null, agents: [] }] }
          : { places: [], selectedKey: null, persistenceState: 'complete' };
        if (command === 'setup_example') return pair;
        if (command === 'setup_connect_model' && window.__bridgeMismatch) throw { kind: 'modelInstallationMismatch', recovery, message: 'fixture-private-diagnostic' };
        if (name === 'ai-reset' && command === 'setup_reset_model') {
          window.__bridgeResets.push(args ?? {});
          const outcome = window.__bridgeResetOutcome;
          if (outcome === 'pending') return new Promise(resolve => { window.__bridgeFinishReset = () => { window.__bridgeMismatch = false; resolve({ ready: false }); }; });
          if (outcome === 'credential') throw { kind: 'modelStore', recovery: { ...recovery, resetStage: 'credentialRemovalUnconfirmed' }, message: 'fixture-private-diagnostic' };
          if (outcome === 'record') throw { kind: 'modelUnavailable', recovery: { ...recovery, resetStage: 'recordRemovalUnconfirmed' } };
          window.__bridgeMismatch = false; return { ready: false };
        }
        if (command === 'setup_connect_model') {
          if (name === 'ai-connecting') return new Promise(() => {});
          if (name === 'ai-cli-missing') throw { kind: 'modelCaptureUnavailable' };
          connected = true; return { ready: true, connection: { kind: 'copilot', model: 'gpt-5.6-terra', account: ${JSON.stringify(fixtureAccount)} } };
        }
        if (command === 'overview_files') return { state: 'complete', runActive: false, historyState: 'complete', runAgents: [], files: [{ documentId, name: documentName, state: 'readable', source, summary: { name: documentName, description: null, model: 'fixture-model', tools: [] }, reason: null, localResult: null }] };
        if (command === 'overview_agents' || command === 'overview_places') { window.__bridgeOverviewLiveReads++; throw Error('Synthetic live inventory unavailable'); }
        if (command === 'agent_open' && name === 'designer-loading') return new Promise(() => {});
        if (command === 'agent_open' && name === 'designer-open-error') throw Error('Synthetic agent open failure');
        if (designer && command === 'agent_select') {
          if (args?.documentId !== documentId) throw Error('Wrong selected document');
          return null;
        }
        if (designer && command === 'agent_selected') return documentId;
        if (designer && (command === 'workspace_catalog' || command === 'cluster_catalog')) {
          window.__bridgeCatalogCalls.push({ command, args });
          if (name === 'designer-catalog-busy' && !window.__bridgeCatalogReleased) throw 'catalogBusy';
          return { defaultModel: name === 'designer-connected' ? 'gpt-5.6-terra' : 'fixture-model', models: { state: 'ready', items: [{ id: name === 'designer-connected' ? 'gpt-5.6-terra' : 'fixture-model' }, ...command === 'workspace_catalog' ? [{ id: 'other-model' }] : []] }, tools: { state: 'ready', items: [{ kind: 'builtin', name: 'web_fetch', description: 'Fetch a page' }, { kind: 'builtin', name: 'request_approval', description: 'Ask a person to approve an action.' }, { kind: 'builtin', name: 'reply_in_conversation', description: 'Send a reply in the current conversation.' }] } };
        }
        if (name === 'designer-remote' && command === 'cluster_contexts') return [context];
        if (name === 'designer-remote' && command === 'cluster_discover') return [target];
        if (command === 'cluster_use') { window.__bridgeClusterUses++; if (name === 'designer-remote') return { status: 'connected', providers: [provider], failure: null }; }
        // A complete idle snapshot, so the blocked-Chat check proves Ready is suppressed, not merely that no status loaded.
        if (designer && !name.startsWith('chat-') && command === 'agent_chat_status') return { documentId, state: 'ready', busy: false, canSend: true, versions: [], error: null };
        if (command === 'send_status') return { documentId, view: null, busy: null };
        if (command === 'agent_create') {
          if (args.name !== 'native-helper') throw Error('Wrong fixture creation name');
          // Example rebinds only for a new identity; all document replies follow it.
          documentId = '00000000-0000-4000-8000-000000000005';
          documentName = args.name;
          pair.agent = ${JSON.stringify('---\nname: "native-helper"\nmodel: "fixture-model"\nkmx:\n  schemaVersion: 1\n  runtime: orka\n---\n\n')};
          pair.tests = ${JSON.stringify('---\nschemaVersion: 1\n---\n\n# native-helper tests\n\n```yaml agent-test\nid: first-test\ninput: ""\nassertions: []\n```\n')};
          return documentId;
        }
        if (command === 'agent_chat_status') {
          const confirming = name === 'chat-session-null' || name === 'chat-session-answer';
          const waiting = name === 'chat-pending' || name === 'chat-running';
          const turn = { message: 'What did the page say?', answer: waiting || name === 'chat-session-null' ? null : 'The page says fixture-checksum.', phase: waiting ? name === 'chat-pending' ? 'Pending' : 'Running' : 'Succeeded', tools: [], contextRetained: !waiting && !confirming, terminationUnconfirmed: false, stopUnconfirmed: false };
          return { documentId, state: confirming ? 'waitingForSession' : waiting ? 'working' : 'ready', busy: waiting || confirming, canSend: !waiting && !confirming, versions: name === 'chat-empty' ? [] : [{ turns: [turn] }], error: null };
        }
        if (name.startsWith('cluster-') && command === 'cluster_contexts') return [context, { ...context, id: '00000000-0000-4000-8000-000000000004', name: 'team-west', current: false }];
        if (name.startsWith('cluster-') && command === 'cluster_discover') {
          window.__bridgeClusterUses++;
          if (name === 'cluster-picker-open') throw Error('Opening picker must not discover');
          if (name === 'cluster-checking') return new Promise(() => {});
          throw { check: 'clusterApi', result: 'connectionFailed' };
        }
        if (name === 'designer-remote' && command === 'send_preflight') return { documentId, targetId: target.id, providerId: provider.id, source, model: 'fixture-model', canSend: true, action: 'create', canSeparate: false, rows: [{ label: 'Cluster', status: 'allowed', message: 'Ready.' }] };
        // In-memory only: lets this fixture's existing editor path save a removal and its restore.
        if (name === 'designer-reply-saved' && command === 'agent_save') { pair.agent = args.content; return { ...pair, documentId, name: 'web-reader' }; }
        if (command === 'agent_open') return { ...pair, documentId, name: documentName, ...(name.startsWith('first-test-') && name !== 'first-test-result' && name !== 'first-test-running' || ['designer-connected', 'designer-remote', 'designer-stale'].includes(name) || localCancel ? { source } : {}) };
        if (localCancel && (command === 'agent_run_cancel' || command === 'agent_run_check_cancellation')) {
          if (args?.documentId !== documentId) throw Error('Wrong cancellation document');
          window.__bridgeCancelCalls.push(command); window.__bridgeCancellation = 'cancelling'; return snapshot();
        }
        if (command === 'agent_run') {
          if (args?.bundledExample !== true) throw Error('Fixture automatic test requires native source guard'); if (submitted) throw Error('Fixture automatic test replayed'); submitted = true; attemptId = args?.attemptId ?? null; window.__bridgeRuns++;
          // Native accepted the run, but its reply never reached the renderer.
          if (name === 'first-test-lost-reply') throw Error('Synthetic lost run reply');
          return snapshot();
        }
        if (command === 'agent_run_status') return snapshot();
        if (command === 'send_history' || command === 'cluster_contexts') return [];
        throw Error('Unmocked action fixture command: ' + command);
      } };
      document.getElementById('root').replaceWith(Object.assign(document.createElement('div'), { id: 'root' }));
    })()`);
    await evaluate(bundle);
    // VM-ready offers Connect the AI, not Manage. This click only opens local detail.
    if (first || preflight) await clickAction('Set up');
    else if (!places) await clickAction('Connect the AI');
    if (first) await clickAction('Continue');
    if (first || ['ai-cli-missing', 'ai-connecting', 'ai-connected', 'ai-reset', 'local-completed', 'local-stopped', 'local-stop-failed', 'local-stop-lost-reply', 'ai-removal-confirmation', 'ai-removal-retry'].includes(name)) await clickAction('Connect the AI');
    if (['local-stopped', 'local-stop-failed', 'local-stop-lost-reply'].includes(name)) {
      await captureWorkspace(theme, size, name);
      return;
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
      await evaluate(`document.querySelector('button[aria-label="Clusters"]').click()`);
      await clickAction('Add a cluster');
    }
    if (name === 'designer-remote') {
      await evaluate(`document.querySelector('button[aria-label="Clusters"]').click()`); await clickAction('Add a cluster');
      await chooseContext('team-dev');
      await clickAction('Use this cluster');
      await clickAction('Open your agent');
    } else if (name.startsWith('designer') || name.startsWith('local-cancel-') || name.startsWith('chat-') || name.startsWith('new-agent-')) await clickAction('Read its instructions');
    if (name.startsWith('chat-')) {
      await clickAction('Chat');
      const expected = name === 'chat-empty' ? 'Ask your agent a question' : name === 'chat-pending' ? 'Waiting for your agent to start…' : name === 'chat-running' ? 'Waiting for your agent’s reply…' : name === 'chat-session-null' ? 'Waiting for the conversation to finish saving…' : 'The page says fixture-checksum.';
      await until(() => evaluate(`document.querySelector('.chat-panel')?.innerText.includes(${JSON.stringify(expected)})`), name);
      if (['chat-pending', 'chat-running', 'chat-session-null', 'chat-session-answer'].includes(name) && await evaluate(`['not confirmed in later context', 'No answer was received.', 'failed without an answer'].some(text => document.querySelector('.chat-transcript').innerText.includes(text))`)) throw Error('Waiting Chat shows a premature missing-answer/context fault');
      if (name.startsWith('chat-session-') && !await evaluate(`document.querySelector('.chat-panel').innerText.includes('Waiting for the conversation to finish saving…')`)) throw Error('Session confirmation lost its pending status');
      await evaluate(`document.querySelector('.chat-panel').scrollIntoView({ block: 'start' })`);
    }
    if (name.startsWith('new-agent-')) {
      // Creation must leave File/Chat and open the blank Instructions editor.
      await clickAction('File');
      await clickAction('New agent');
      await evaluate(`(() => { const input = document.querySelector('#new-agent-name'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'native-helper'); input.dispatchEvent(new Event('input', { bubbles: true })); })()`);
      if (name === 'new-agent-created') {
        await clickAction('Create');
        // The old editor briefly reappears before selection rebinds; wait for the new document too.
        await until(() => evaluate(`document.querySelector('.agent-selector select:not(:disabled)')?.value === '00000000-0000-4000-8000-000000000005' && !!document.querySelector('.instructions-editor') && !document.querySelector('.new-agent-form')`), 'created agent Instructions editor');
        if (await evaluate(`document.querySelector('.instructions-editor').value !== '' || document.body.innerText.includes('Tests could not be read')`)) throw Error('Blank draft was seeded or shown as unreadable');
        if (await evaluate(`document.querySelector('.agent-selector select').selectedOptions[0]?.textContent !== 'native-helper'`)) throw Error('Created agent selector must name native-helper');
      }
      await evaluate(`document.querySelector('main').scrollTop = 0`);
    }
    if (clusterScreen) {
      await clickAction('Add a cluster');
      if (name === 'cluster-picker-open') {
        await until(() => evaluate(`!!document.querySelector('button#cluster-context:not(:disabled)')`), 'context picker');
        await evaluate(`document.querySelector('#cluster-context').click()`);
        await until(() => evaluate(`!!document.querySelector('[role="dialog"][aria-label="Choose a context"]')`), 'open context dialog');
        if (await evaluate('window.__bridgeClusterUses') !== 0) throw Error('Picker opened a background discovery');
      } else {
        await chooseContext('team-dev');
        await until(() => evaluate(name === 'cluster-checking' ? `document.querySelector('.cluster-card')?.getAttribute('aria-busy') === 'true' && window.__bridgeClusterUses === 1` : `!!document.querySelector('.cluster-card .cluster-failure')`), name);
      }
    }
    if (name.startsWith('designer') && name !== 'designer-loading' && name !== 'designer-open-error') {
      await until(() => evaluate(`!!document.querySelector('.instructions-editor')`), 'Designer editor');
      if (name === 'designer-catalog-busy') {
        await until(() => evaluate(`document.body.innerText.includes('Loading available tools and models') && window.__bridgeCatalogCalls.length >= 2`), 'busy first catalog stays loading');
        if (await evaluate(`document.body.innerText.includes('Tools could not be read') || [...document.querySelectorAll('button')].some(button => button.textContent === 'Refresh tools and models')`)) throw Error('Busy catalog was shown as a failed load');
        const loading = await call('Page.captureScreenshot', { format: 'png' });
        await writeFile(join(output, `${theme}-${size}-catalog-first-loading.png`), Buffer.from(loading.data, 'base64'));
        await evaluate(`window.__bridgeCatalogReleased = true`);
      }
      if (name !== 'designer') await until(() => evaluate(`!document.querySelector('.model-select').disabled`), 'connected Designer catalog');
      await until(() => evaluate(`document.querySelector('#tab-status-Overview')?.textContent === '1 saved agent'`), 'hidden saved-file count');
      if (await evaluate(`window.__bridgeOverviewLiveReads !== 0 || /\\bSaved\\b/.test(document.querySelector('footer').innerText)`)) throw Error('Untouched Designer claimed Saved or hidden Overview read live inventory');
      if (name === 'designer-catalog-busy') {
        await evaluate(`document.querySelector('button[aria-controls$="-picker"]').click()`);
        await until(() => evaluate(`!!document.querySelector('.tool-picker')?.getClientRects().length`), 'first tool list without Refresh');
        const firstList = await call('Page.captureScreenshot', { format: 'png' });
        await writeFile(join(output, `${theme}-${size}-catalog-first-open.png`), Buffer.from(firstList.data, 'base64'));
        await evaluate(`document.querySelector('button[aria-label="Close picker"]').click()`);
      }
      if (name === 'designer-file') await clickAction('File');
      if (name === 'designer-failed') await until(() => evaluate(`!!document.querySelector('.test-next-step')`), 'failure next action');
      const state = await evaluate(`({ historyHidden: !document.querySelector('.designer-checks .send-history')?.getClientRects().length, catalog: window.__bridgeCatalogCalls, runPrimary: [...document.querySelectorAll('.example-run button')].some(button => button.textContent === 'Run tests' && button.classList.contains('button-primary')) })`);
      if (await evaluate(`document.body.innerText.includes('Your agent selection could not be saved')`)) throw Error('Designer fixture failed to acknowledge document selection');
      if (name === 'designer' && (state.catalog.length || state.runPrimary)) throw Error('Disconnected Designer presents a runnable primary/catalog: ' + JSON.stringify(state));
      // Send is retained but hidden until used or a cluster is selected; its header entry and Past Sends section are always present.
      const sendShown = await evaluate(`!!document.querySelector('.send-panel')?.getClientRects().length`);
      if (sendShown !== (name === 'designer-remote')) throw Error(`${name}: Send card visibility is wrong (visible=${sendShown})`);
      if (!await evaluate(`!!document.querySelector('.send-panel') && !!document.querySelector('.agent-selector select') && [...document.querySelectorAll('.send-entry button')].some(b => b.textContent === 'Send to a cluster')`)) throw Error(`${name}: retained Send, header picker or Send entry is missing`);
      if (!sendShown && await evaluate(`[...document.querySelectorAll('.designer-checks h2')].some(h => h.getClientRects().length)`)) throw Error(`${name}: hidden Send left its team heading`);
      if (await evaluate(`document.querySelector('.send-history details')?.open !== false`)) throw Error(`${name}: Past Sends must start closed`);
      if (name === 'designer-remote' && (state.historyHidden || !state.catalog.some(call => call.command === 'cluster_catalog' && call.args.targetId === '00000000-0000-4000-8000-000000000002' && call.args.providerId === '00000000-0000-4000-8000-000000000003'))) throw Error('Remote catalog is not scoped to the selected cluster/AI');
    }
    if (name.startsWith('local-cancel-')) {
      const state = name.slice('local-cancel-'.length);
      const label = { running: 'Running', cancelling: 'Cancelling', cancelled: 'Cancelled', unconfirmed: 'Not confirmed', 'before-start': 'Cancelled' }[state];
      await until(() => evaluate(`document.querySelector('.example-run .pill')?.textContent === ${JSON.stringify(label)}`), name);
      const probe = await evaluate(`(() => {
        const card = document.querySelector('.example-run');
        return { buttons: [...card.querySelectorAll('button')].map(b => b.textContent.trim()), text: card.innerText, rows: [...card.querySelectorAll('.test-result > summary code')].map(c => c.textContent), width: innerWidth, scrollWidth: document.querySelector('main').scrollWidth };
      })()`);
      if (probe.scrollWidth > probe.width + 2) throw Error('Cancellation layout overflows: ' + JSON.stringify(probe));
      if (state === 'running' && !probe.buttons.includes('Stop')) throw Error('Missing local Stop');
      if (state === 'cancelling' && probe.buttons.includes('Stop')) throw Error('Cancelling is a status, not a button');
      if (state === 'cancelled' && (probe.rows.join('|') !== 'finished-answer|unfinished-test' || !probe.text.includes('1 passed, 0 failed, 1 cancelled.'))) throw Error('Lost results/order/counts: ' + JSON.stringify(probe));
      if (state === 'unconfirmed' && !['Retry cancellation', 'Check again', 'Stop the local cluster'].every(b => probe.buttons.includes(b))) throw Error('Missing cancellation recovery');
      if (state === 'before-start' && (!probe.text.includes('Cancelled before any test started.') || probe.rows.length)) throw Error('Incorrect before-start result');
      // Open the kept answer and capture the card in the real native WebView.
      if (state === 'cancelled') await evaluate(`document.querySelector('.example-run .test-result').open = true`);
      await evaluate(`document.querySelector('.example-run').scrollIntoView({ block: 'start' })`);
      await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
      const image = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${size}-${name}.png`), Buffer.from(image.data, 'base64'));
      if (await evaluate('window.__bridgeCancelCalls.length') !== 0) throw Error('Cancellation replayed on mount');
      if (state === 'running') {
        await clickAction('Stop');
        await until(() => evaluate(`document.querySelector('.example-run .pill')?.textContent === 'Cancelling' && window.__bridgeCancelCalls.join() === 'agent_run_cancel'`), 'explicit native renderer Cancel');
      }
      if (state === 'unconfirmed') {
        for (const [button, command] of [['Check again', 'agent_run_check_cancellation'], ['Retry cancellation', 'agent_run_cancel']]) {
          await clickAction(button);
          await until(() => evaluate(`document.querySelector('.example-run .pill')?.textContent === 'Cancelling' && window.__bridgeCancelCalls.at(-1) === ${JSON.stringify(command)}`), button);
          await evaluate(`window.__bridgeCancellation = 'unconfirmed'`);
          await until(() => evaluate(`document.querySelector('.example-run .pill')?.textContent === 'Not confirmed'`), 'recovery back to unconfirmed');
        }
        // Scope the click to Tests because Setup's hidden Stop stays mounted.
        await evaluate(`[...document.querySelectorAll('.example-run button')].find(b => b.textContent === 'Stop the local cluster').click()`);
        await until(() => evaluate(`window.__bridgeStops === 1 && document.querySelector('button[aria-label="Clusters"]').getAttribute('aria-current') === 'page' && [...document.querySelectorAll('button')].some(b => b.textContent === 'Start' && b.getClientRects().length)`), 'explicit local Stop');
      }
      console.log(`Captured native WebView2 local cancellation fixture: ${theme}/${size}/${name}`);
      return;
    }
    const settle = () => evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
    const visibleText = text => `[...document.querySelectorAll('h1, p, span, button')].some(node => node.getClientRects().length && node.textContent === ${JSON.stringify(text)})`;
    if (name === 'first-test-passed') {
      await until(() => evaluate(visibleText('Your first agent passed its test')), 'first-pass moment').catch(async error => { throw Error(`${error.message}: ${await evaluate('document.body.innerText')}`); });
      const probe = await evaluate(`(() => {
        const card = document.querySelector('.first-pass');
        const shown = [...document.querySelectorAll('button')].filter(button => button.getClientRects().length);
        return { page: document.querySelector('button[aria-label="Clusters"]').getAttribute('aria-current'), primary: shown.filter(button => button.classList.contains('button-primary')).map(button => button.textContent), stop: shown.some(button => button.textContent === 'Stop the local cluster'), text: card?.innerText ?? '', runs: window.__bridgeRuns, width: innerWidth, scrollWidth: document.querySelector('main').scrollWidth };
      })()`);
      const fail = reason => { throw Error(`${name}: ${reason}: ${JSON.stringify(probe)}`); };
      if (probe.page !== 'page') fail('a passed automatic test stays in Clusters');
      if (probe.primary.join('|') !== 'Open web-reader' || probe.stop) fail('the payoff has one primary Open web-reader and no Stop');
      if (!['What checksum does the page show?', 'fixture-checksum', 'The test passed.'].every(text => probe.text.includes(text))) fail('the recorded TestStory is missing');
      if (probe.runs !== 1 || probe.scrollWidth > probe.width + 2) fail('one run, no horizontal overflow');
      await checkLayout(theme, size, name);
      await settle();
      const image = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${size}-${name}.png`), Buffer.from(image.data, 'base64'));
      await compareReference(image.data, theme, size, name, 'first-pass');
      await clickAction('Open web-reader');
      await until(() => evaluate(`document.querySelector('button[aria-label="Designer"]').getAttribute('aria-current') === 'page'`), 'explicit Open web-reader');
      await evaluate(`document.querySelector('button[aria-label="Clusters"]').click()`);
      await until(() => evaluate(`document.querySelector('button[aria-label="Clusters"]').getAttribute('aria-current') === 'page'`), 'back to Clusters');
      if (await evaluate(`!!document.querySelector('.first-pass')?.getClientRects().length || window.__bridgeRuns !== 1`)) throw Error(`${name}: the first-pass moment or run replayed`);
      console.log(`Captured native WebView2 first-pass fixture: ${theme}/${size}/${name}`);
      return;
    }
    if (name === 'first-test-unknown' || name === 'first-test-lost-reply') {
      // Unknown is never green; a lost reply may pass but cannot prove this attempt owned the result.
      const label = name === 'first-test-unknown' ? 'Result unknown' : 'Its test passed';
      await until(() => evaluate(`[...document.querySelectorAll('[aria-label="Setup steps"] > li')].find(row => row.querySelector('b')?.textContent === 'Run your first test')?.querySelector('.step-result')?.textContent === ${JSON.stringify(label)}`), `${name} settled`);
      await evaluate(`document.querySelector('button[aria-label="Clusters"]').click()`);
      await until(() => evaluate(`document.querySelector('button[aria-label="Clusters"]').getAttribute('aria-current') === 'page'`), 'Clusters after automatic test');
      const probe = await evaluate(`({ payoff: !!document.querySelector('.first-pass'), green: !!document.querySelector('.setup-progress .steps-head .status-passed') && ${JSON.stringify(name === 'first-test-unknown')}, runs: window.__bridgeRuns, text: document.querySelector('[aria-label="Local cluster setup"]')?.innerText })`);
      if (probe.payoff || probe.green || probe.runs !== 1) throw Error(`${name}: must not celebrate or replay: ${JSON.stringify(probe)}`);
      await evaluate(`document.querySelector('[aria-label="Local cluster setup"]').scrollIntoView({ block: 'start' })`);
      await settle();
      const image = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${size}-${name}.png`), Buffer.from(image.data, 'base64'));
      console.log(`Captured native WebView2 first-test fixture: ${theme}/${size}/${name}`);
      return;
    }
    if (name === 'ai-key') {
      // Choose the key surface only; no key is typed, stored or checked.
      await evaluate(`[...document.querySelectorAll('.setup-model label')].find(label => label.textContent.trim().startsWith('Your own API key')).querySelector('input').click()`);
      await until(() => evaluate(`!!document.querySelector('#model-key-help')`), 'key custody copy');
      const key = await evaluate(`({ help: document.querySelector('#model-key-help').textContent, model: [...document.querySelectorAll('.setup-model code')].map(code => code.textContent), text: document.querySelector('.setup-model').innerText, pickers: document.querySelectorAll('.setup-model select, .setup-model [role=combobox]').length })`);
      if (!key.help.includes('credential store') || !key.model.includes('claude-sonnet-5-5') || key.pickers || /Keychain|Credential Manager|Azure|local cluster/.test(key.text)) throw Error(`${name}: key copy must be neutral, name the fixed model and offer no picker: ${JSON.stringify(key)}`);
    }
    if (name === 'first-test-badges') {
      await until(() => evaluate(`!!document.querySelector('.designer-work')`), 'sample result before badge recovery');
      await evaluate(`document.querySelector('button[aria-label="Clusters"]').click()`);
      await until(() => evaluate(`document.querySelectorAll('[aria-label="Setup steps"] > li').length === 4`), 'fixed completed setup steps');
      // Result actions belong to the same fixed setup card as the four rows.
      for (const [state, label, primary] of [['passed', 'Its test passed', 'See what it answered'], ['failed', 'Its test failed', 'See what it answered'], ['stale', 'Needs running', 'Run it again'], ['unknown', 'Result unknown', 'Check again']]) {
        await evaluate(`window.__bridgeBadgeState = ${JSON.stringify(state)}`);
        await until(() => evaluate(`[...document.querySelectorAll('[aria-label="Setup steps"] > li')].find(row => row.querySelector('b')?.textContent === 'Run your first test')?.querySelector('.step-result')?.textContent === ${JSON.stringify(label)}`), `sample badge ${state}`);
        const card = await evaluate(`(() => {
          const card = document.querySelector("[aria-label='Local cluster setup']");
          const shown = [...card.querySelectorAll('button')].filter(button => button.getClientRects().length);
          return { text: card.innerText, primary: shown.filter(button => button.classList.contains('button-primary')).map(button => button.textContent), result: shown.some(button => button.textContent === 'See what it answered'), page: document.body.innerText };
        })()`);
        const fail = reason => { throw Error(`sample ${state}: ${reason}: ${JSON.stringify(card)}`); };
        if (card.primary.join('|') !== primary) fail(`primary action should be ${primary}`);
        if (/step \d+ of [45]/i.test(card.page)) fail('completed test evidence is shown as unfinished setup');
        if (state === 'stale' && !card.result) fail('the stale sample has a readable report, so its result stays inspectable');
        if (card.text.includes('Its test failed') !== (state === 'failed')) fail('failure message belongs only to the failed test');
        for (const [suffix, target] of [['', '[aria-label="Setup steps"] > li:last-child'], ['-card', '[aria-label="Local cluster setup"]']]) {
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
    if (name === 'ai-reset') {
      // One mount walks frames 1g/1h, so the added states stay within the fixed CI budget.
      const row = '[aria-label="Setup steps"] > li:nth-child(3) .model-recovery';
      const probe = scope => evaluate(`(() => {
        const root = document.querySelector(${JSON.stringify(scope)});
        const shown = [...document.querySelectorAll('button')].filter(b => b.getClientRects().length).map(b => b.textContent.trim());
        return { inScope: !!root?.getClientRects().length, text: root?.innerText ?? '', heading: document.querySelector('h1').textContent, page: document.body.innerText, buttons: shown,
          primary: [...(root?.querySelectorAll('.button-primary') ?? [])].map(b => b.textContent), resets: window.__bridgeResets };
      })()`);
      const expect = async (state, scope, check) => {
        let seen;
        await until(async () => { seen = await probe(scope); return seen.inScope && check(seen); }, `ai-reset ${state}`).catch(error => { throw Error(`${error.message}: ${JSON.stringify(seen)}`); });
        if (seen.page.includes('fixture-private-diagnostic') || seen.resets.some(args => Object.keys(args).length)) throw Error(`ai-reset ${state}: private text shown or reset given renderer arguments`);
        return seen;
      };
      const shoot = async (state, frame, target) => {
        await evaluate(`document.querySelector(${JSON.stringify(target)}).scrollIntoView({ block: 'center' })`);
        await checkLayout(theme, size, `ai-reset-${state}`);
        await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
        const image = await call('Page.captureScreenshot', { format: 'png' });
        await writeFile(join(output, `${theme}-${size}-ai-reset-${state}.png`), Buffer.from(image.data, 'base64'));
        await compareReference(image.data, theme, size, `ai-reset-${state}`, frame);
      };
      const mismatch = s => s.text.includes("Bridge's saved AI connection is from an earlier setup.") && s.primary.join() === 'Reset AI connection';
      await expect('mismatch', row, s => mismatch(s) && s.heading === 'This computer' && s.page.includes("The AI couldn't connect") && s.page.includes('Your workspace is running. Nothing is lost.')
        && ['Use another provider', 'Copy details for help'].every(b => s.buttons.includes(b)) && !['Remove sign-in', 'Check again', 'Connect the AI'].some(b => s.buttons.includes(b)));
      await shoot('mismatch', 'failed', row);
      await evaluate(`document.querySelector('button[aria-label="Clusters"]').click()`);
      await expect('place', '[aria-label="This computer"] .model-recovery', s => mismatch(s) && s.page.includes('Workspace running'));
      await shoot('place', 'failed', '[aria-label="This computer"]');
      await clickAction('Connect the AI');
      await expect('return', row, mismatch);
      await clickAction('Reset AI connection');
      await expect('confirmation', row, s => s.text.includes('Reset the AI connection?') && s.text.includes('Use another provider starts with this same reset.') && ['Reset', 'Cancel'].every(b => s.buttons.includes(b)) && !s.resets.length);
      await shoot('confirmation', 'reset', row);
      await clickAction('Cancel');
      await expect('cancelled', row, s => mismatch(s) && !s.resets.length);
      await clickAction('Use another provider');
      await expect('provider confirmation', row, s => s.text.includes('Reset the AI connection?') && !s.resets.length);
      await clickAction('Cancel');
      await expect('provider cancelled', row, s => mismatch(s) && !s.resets.length);
      await clickAction('Reset AI connection'); await clickAction('Reset');
      await expect('failed', row, s => s.heading === 'This computer' && s.page.includes("The AI connection couldn't be reset") && s.text.includes("Bridge couldn't reset the AI connection") && s.primary.join() === 'Reset again'
        && !['Use another provider', 'Remove sign-in'].some(b => s.buttons.includes(b)) && s.resets.length === 1);
      await shoot('failed', 'reset', row);
      await clickAction('Copy details for help');
      await expect('copy-notice', row, s => s.text.includes('Details copied. No keys or sign-in tokens are included.'));
      const copied = await evaluate('window.__bridgeCopied.join()');
      if (!['Operation: Resetting AI connection', 'Reset stage: credentialRemovalUnconfirmed', '10000000-0000-4000-8000-000000000001', '20000000-0000-4000-8000-000000000002', '30000000-0000-4000-8000-000000000003'].every(line => copied.includes(line)) || copied.includes('fixture-private-diagnostic')) throw Error('Unsafe or incomplete copied details: ' + copied);
      await shoot('copy-notice', 'reset', row);
      await evaluate(`document.querySelector(${JSON.stringify(row + ' summary')}).click()`);
      await expect('details', row, s => s.text.includes('Native credential removal not confirmed; saved record kept.') && s.text.includes('30000000-0000-4000-8000-000000000003'));
      await shoot('details', 'reset', row);
      await evaluate(`window.__bridgeResetOutcome = 'record'`);
      await clickAction('Reset again');
      await expect('failed-again', row, s => s.text.includes('Reset failed again') && s.text.includes('Native credential removed; saved record removal not confirmed.') && s.resets.length === 2);
      await shoot('failed-again', 'reset', row);
      await evaluate(`window.__bridgeResetOutcome = 'pending'`);
      await clickAction('Reset again');
      await expect('resetting', row, s => s.heading === 'This computer' && s.page.includes('Resetting the AI connection') && s.text.includes('Resetting') && !['Remove sign-in', 'Connect the AI', 'Reset again', 'Reset AI connection'].some(b => s.buttons.includes(b)));
      await shoot('resetting', 'reset', row);
      await evaluate('window.__bridgeFinishReset()');
      await expect('done', '.setup-model', s => s.text.includes('Connection reset. Choose the AI again.') && s.heading === 'This computer' && s.page.includes('Connect the AI') && s.buttons.includes('Connect the AI') && !s.buttons.includes('Remove sign-in') && s.resets.length === 3);
      await shoot('done', 'reset', '.setup-model');
      console.log(`Captured native WebView2 AI reset fixture: ${theme}/${size}`);
      return;
    }
    const expected = { welcome: 'About 5 minutes, mostly a download', 'chat-empty': 'Ask your agent a question', 'chat-pending': 'Waiting for your agent to start…', 'chat-running': 'Waiting for your agent’s reply…', 'chat-answer': 'The page says fixture-checksum.', 'chat-session-null': 'Waiting for the conversation to finish saving…', 'chat-session-answer': 'The page says fixture-checksum.', 'new-agent-form': 'Starts blank, with one empty test.', 'new-agent-created': 'Your first test is incomplete.', 'cluster-picker-open': 'Find a context', 'cluster-checking': 'Checking the cluster', 'cluster-failure': "Couldn't reach this cluster.", 'places-empty': 'No other clusters added yet', 'places-populated': 'Not checked yet', 'local-preflight': 'Step 1 of 4', 'local-preflight-missing': 'Step 1 of 4', 'local-preflight-unknown': "Bridge couldn't check this computer yet.", 'local-preflight-blocked': 'Fix the computer requirements above', 'local-preflight-memory': 'Fix the computer requirements above', 'local-preflight-mixed': 'Fix the computer requirements above', 'local-preflight-cluster': 'Fix the computer requirements above', 'ai-cli-missing': 'gh auth login --hostname github.com --web', 'designer-loading': 'Opening your agent', 'designer-open-error': 'Your agent folder could not be opened.', 'ai-key': 'Where do I get a key?', 'ai-idle': 'GitHub CLI must be installed and signed in', 'ai-connecting': 'Connecting', 'ai-connected': 'Connected', 'first-test-running': 'Run your first testRunning', 'first-test-result': 'No checksum was given.', 'local-completed': 'Local cluster ready', 'ai-removal-confirmation': 'Disconnect the AI and remove Bridge’s saved sign-in?', 'ai-removal-retry': 'Press Retry removal', 'switch-place': 'Add a cluster', designer: 'Instructions and settings', 'designer-connected': 'Instructions and settings', 'designer-catalog-busy': 'Instructions and settings', 'designer-file': 'web-reader.agent.md', 'designer-reply-saved': 'Reply in a conversation', 'designer-failed': 'checks failed', 'designer-passed': 'The test passed.', 'designer-stale': 'Not run since your edit', 'designer-running': 'Waiting for answers and evaluating tests', 'designer-remote': 'Instructions and settings' };
    await until(() => evaluate(`document.body.textContent.includes(${JSON.stringify(expected[name])}) && ${['designer-loading', 'designer-open-error'].includes(name) ? "!!document.querySelector('.designer > .designer-file')?.getClientRects().length" : 'true'}`), `${name} action state`).catch(async error => {
      throw Error(`${error.message}: ${await evaluate('document.body.innerText')}`);
    });
    if (name === 'welcome') {
      const welcome = await evaluate(`(() => {
        const root = document.querySelector('.welcome-screen');
        const start = [...root.querySelectorAll('button')].find(button => button.textContent === 'Get started');
        return { text: root.innerText, primary: start?.classList.contains('button-primary'), expert: [...root.querySelectorAll('button')].some(button => button.textContent.includes('I already run Orka on a cluster')) };
      })()`);
      if (!welcome.primary || !welcome.expert || !/team clusters are early in this preview/i.test(welcome.text)) throw Error('Welcome lost its primary, expert route or preview caveat: ' + JSON.stringify(welcome));
    }
    if (preflight) {
      if (name !== 'local-preflight-missing') await until(() => evaluate(`document.querySelector('[aria-label="Computer requirements"]')?.getAttribute('aria-busy') === 'false'`), 'settled computer checks');
      const probe = await evaluate(`(() => {
        const root = document.querySelector('.local-preflight');
        const checks = root.querySelector('[aria-label="Computer requirements"]');
        const next = [...root.querySelectorAll('button')].find(button => button.textContent === 'Continue');
        const again = [...root.querySelectorAll('button')].find(button => button.textContent === 'Check again');
        return { text: root.innerText, radios: [...root.querySelectorAll('input[type=radio]')].map(radio => ({ value: radio.value, checked: radio.checked })), next: { disabled: next.disabled, primary: next.classList.contains('button-primary') }, again: again && { primary: again.classList.contains('button-primary'), after: !!(next.compareDocumentPosition(again) & Node.DOCUMENT_POSITION_FOLLOWING) }, unknown: checks?.querySelectorAll('.status-not-started').length, openHelp: !!checks?.querySelector('[aria-expanded="true"]') };
      })()`);
      const fail = reason => { throw Error(name + ': ' + reason + ': ' + JSON.stringify(probe)); };
      if (await evaluate(`[...document.querySelectorAll('button')].some(button => ['Back to clusters', 'Choose where agents run'].includes(button.textContent) && button.getClientRects().length)`)) fail('local choice must not have a header Back link');
      if (probe.radios.length !== 2 || !probe.radios[0].checked || probe.radios[1].checked || !/recommended/i.test(probe.text) || !probe.text.includes('On a cluster you already have')) fail('local choice must default to recommended and retain the cluster alternative');
      if (!probe.next.primary || probe.next.disabled !== ['local-preflight-blocked', 'local-preflight-memory', 'local-preflight-mixed', 'local-preflight-cluster'].includes(name)) fail('Continue must be the primary, blocked only by known local failures');
      if (probe.again?.primary) fail('Check again must remain secondary');
      if (name === 'local-preflight-missing' ? !probe.text.includes('not available yet') : !probe.text.includes('4.3 GB') || /GiB|Cluster download/.test(probe.text)) fail('requirements must be explicit, native-derived decimal GB or unavailable');
      if (['local-preflight', 'local-preflight-unknown'].includes(name) && (probe.unknown || probe.again || /Virtualization:|Installed memory:|Not checked/.test(probe.text))) fail('nonfailed checks must not draw rows or retry actions');
      if (['local-preflight-unknown', 'local-preflight-mixed'].includes(name) && (probe.openHelp || !probe.text.includes("Bridge couldn't check this computer yet. It checks again when the workspace starts."))) fail('unknown checks need one quiet startup explanation');
      if (['local-preflight-memory', 'local-preflight-mixed'].includes(name) && (!probe.text.includes('Not enough installed memory') || probe.text.includes('Virtualization:'))) fail('only the failed RAM check gets a row');
      if (name === 'local-preflight-blocked' && !await evaluate(`[...document.querySelectorAll('[aria-label="Virtualization"] [role="button"]')].some(button => button.textContent.includes('Show me how'))`)) fail('known Windows virtualization failure must keep native fix help');
      if (name === 'local-preflight-cluster') {
        await clickAction('Use a cluster instead');
        if (!await evaluate(`document.activeElement?.id === 'workspace-cluster'`)) fail('fallback must focus the cluster radio');
        await until(() => evaluate(`document.querySelector('#workspace-cluster')?.checked && !document.querySelector('[aria-label="Computer requirements"]') && [...document.querySelectorAll('.local-preflight button')].some(button => button.textContent === 'Continue' && !button.disabled)`), 'cluster choice bypasses local blocker');
      }
      if (await evaluate('window.__bridgeLocalSelections || window.__bridgeClusterUses')) fail('opening or choosing a setup route must not start local setup or admit a cluster');
    }
    if (name.startsWith('places-')) {
      // Count visible controls: hidden local panels stay mounted under Places.
      const probe = await evaluate(`(() => {
        const count = label => [...document.querySelectorAll('button')].filter(button => button.textContent.trim() === label && button.getClientRects().length).length;
        return { setUp: count('Set up'), connect: count('Connect'), add: count('Add a cluster'), back: count('Back to clusters'), continue: count('Continue'), radios: [...document.querySelectorAll('input[type=radio]')].filter(radio => radio.getClientRects().length).length, text: document.body.innerText };
      })()`);
      const fail = reason => { throw Error(`${name}: ${reason}: ${JSON.stringify(probe)}`); };
      if (probe.radios || /Where should your agents run|Engine diagnostics|Unavailable in this build|Reading…/.test(probe.text)) fail('removed chooser, diagnostics, caveat or unsettled size is visible');
      if (name === 'places-empty' && (probe.setUp !== 1 || probe.add !== 1 || probe.connect)) fail('empty Places needs only Set up and Add a cluster');
      if (name === 'places-populated' && (probe.setUp !== 1 || probe.connect !== 2 || !probe.text.includes('Last checked') || probe.text.includes('Selected for Designer'))) fail('remembered places need Last checked, Not checked yet and one Connect each');
    }
    await checkLayout(theme, size, name);
    if (name === 'first-test-result' || name.startsWith('designer') && name !== 'designer-loading' && name !== 'designer-open-error') {
      await until(() => evaluate(`!!document.querySelector('.designer-work')`), 'Designer columns');
      const layout = await evaluate(`(() => {
        const tests = document.querySelector('.designer-tests').getBoundingClientRect();
        const editor = document.querySelector('.designer-file').getBoundingClientRect();
        const send = document.querySelector('.designer-checks').getBoundingClientRect();
        return { tests: { x: tests.x, y: tests.y, right: tests.right, bottom: tests.bottom }, editor: { x: editor.x, y: editor.y, right: editor.right, bottom: editor.bottom }, send: { x: send.x, y: send.y, right: send.right, bottom: send.bottom }, sendY: send.y, width: innerWidth, height: innerHeight, scrollWidth: document.querySelector('main').scrollWidth };
      })()`);
      // Local tests and Send share a right-hand stack; compact widths move that stack below the editor.
      if (size === 'wide' ? Math.abs(layout.tests.y - layout.editor.y) > 2 || layout.tests.x < layout.editor.right - 2
        : layout.tests.y < layout.editor.bottom - 2 || Math.abs(layout.tests.x - layout.editor.x) > 2 || Math.abs(layout.tests.right - layout.editor.right) > 2) throw Error(`Test visibility failed: ${JSON.stringify(layout)}`);
      if (Math.abs(layout.send.x - layout.tests.x) > 2 || Math.abs(layout.send.right - layout.tests.right) > 2
        || layout.send.y < layout.tests.bottom - 2 || layout.scrollWidth > layout.width + 2) throw Error(`Editor/Send layout failed: ${JSON.stringify(layout)}`);
      await writeFile(join(output, `${theme}-${size}-${name}-columns.json`), JSON.stringify(layout, null, 2));
    }
    if (['designer-connected', 'designer-running', 'designer-failed', 'designer-passed', 'designer-stale'].includes(name)) await evaluate(`document.querySelector('.example-run').scrollIntoView({ block: 'start' })`);
    if (name.startsWith('ai-removal-')) await evaluate(`document.querySelector('.setup-model').scrollIntoView({ block: 'center' })`);
    await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
    const image = await call('Page.captureScreenshot', { format: 'png' });
    await writeFile(join(output, `${theme}-${size}-${name}.png`), Buffer.from(image.data, 'base64'));
    if (name === 'welcome' || preflight) {
      // The compact loop uses 540×640; preserve the choice and its scrolled action.
      await evaluate(`document.querySelector('main').scrollTop = document.querySelector('main').scrollHeight`);
      await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
      const actionLabel = name === 'welcome' ? 'Get started' : 'Continue';
      const reachable = await evaluate(`(() => {
        const button = [...document.querySelectorAll('button')].find(button => button.textContent === ${JSON.stringify(actionLabel)} && button.getClientRects().length);
        const r = button?.getBoundingClientRect(), main = document.querySelector('main').getBoundingClientRect();
        return !!r && r.top >= main.top && r.bottom <= main.bottom && r.left >= main.left && r.right <= main.right;
      })()`);
      if (!reachable) throw Error(name + '/' + size + ': primary action is not reachable by scrolling');
      const end = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${size}-${name}-end.png`), Buffer.from(end.data, 'base64'));
      if (name === 'welcome') {
        await clickAction('Get started');
        await until(() => evaluate(`document.body.innerText.includes('Choose where agents run') && !document.querySelector('.welcome-screen')`), 'Get started retains Places navigation');
      } else if (name === 'local-preflight-cluster') {
        await clickAction('Continue');
        await until(() => evaluate(`!!document.querySelector('.cluster-card')?.getClientRects().length`), 'existing cluster chooser');
      }
      if (await evaluate('window.__bridgeLocalSelections || window.__bridgeClusterUses')) throw Error(name + ': navigation started local setup or admitted a cluster');
    }
    if (preflight) await compareReference(image.data, theme, size, name, 'setup-choices');
    if (['designer-connected', 'designer-running', 'designer-failed', 'designer-passed', 'designer-stale'].includes(name)) await compareDesigner(image.data, theme, size, name);
    if (['ai-idle', 'ai-key', 'ai-connected'].includes(name)) await compareReference(image.data, theme, size, name, 'ai');
    if (name.startsWith('places-') || clusterScreen) await compareReference(image.data, theme, size, name, 'clusters');
    if (name.startsWith('chat-') || name.startsWith('new-agent-')) await compareDesigner(image.data, theme, size, name);
    if (size !== 'wide' && ['new-agent-form', 'cluster-failure'].includes(name)) {
      await evaluate(`document.querySelector('main').scrollTop = document.querySelector('main').scrollHeight`);
      await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
      const labels = name === 'new-agent-form' ? ['Create', 'Cancel'] : ['Check again'];
      const reachable = await evaluate(`(() => {
        const main = document.querySelector('main').getBoundingClientRect();
        return ${JSON.stringify(labels)}.every(label => [...document.querySelectorAll('button')].some(button => {
          const r = button.getBoundingClientRect();
          return button.textContent === label && !button.disabled && r.width > 0 && r.top >= main.top && r.bottom <= main.bottom;
        }));
      })()`);
      if (!reachable) throw Error(name + ': recovery/creation actions overlap fixed chrome');
      const end = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${size}-${name}-end.png`), Buffer.from(end.data, 'base64'));
      await compareReference(end.data, theme, size, name + '-end', clusterScreen ? 'clusters' : 'designer');
    }
    if (name === 'designer-passed') {
      await evaluate(`document.querySelector('.example-run .test-result > summary').click()`);
      await until(() => evaluate(`document.querySelector('.example-run').innerText.includes('Read a web page')`), 'passed story opens recorded tool evidence');
      const story = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${size}-${name}-story.png`), Buffer.from(story.data, 'base64'));
      await compareDesigner(story.data, theme, size, name + '-story');
      await evaluate(`document.querySelector('button[aria-controls$="-picker"]').click()`);
      await until(() => evaluate(`!!document.querySelector('.tool-picker')?.getClientRects().length`), 'tool picker');
      const blocked = await evaluate(`(() => {
        const picker = document.querySelector('.tool-picker');
        const approval = [...picker.querySelectorAll('button')].find(button => button.textContent.includes('request_approval'));
        const reason = approval?.parentElement.querySelector('.tool-unavailable-reason');
        const r = reason?.getBoundingClientRect(), p = picker.getBoundingClientRect();
        return approval?.disabled && reason?.textContent === 'Approval tools need autonomous coordination, which Bridge does not support for local tests or Chat.'
          && !approval.contains(reason) && approval.parentElement.tabIndex === 0
          && approval.parentElement.getAttribute('aria-describedby') === reason.id
          && r.top >= p.top && r.bottom <= p.bottom && r.left >= p.left && r.right <= p.right;
      })()`);
      if (!blocked) throw Error('Unsupported approval tool is offered without its reason');
      const pickerImage = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${size}-approval-tool.png`), Buffer.from(pickerImage.data, 'base64'));
      // Filter after the approval frame so the reply row and any reason must fit even at minimum size.
      await evaluate(`(() => { const input = document.querySelector('.tool-picker input[type="search"]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'reply'); input.dispatchEvent(new Event('input', { bubbles: true })); })()`);
      await until(() => evaluate(`document.querySelectorAll('.tool-picker li button').length === 1`), 'reply filter');
      const reply = await evaluate(`(() => {
        const picker = document.querySelector('.tool-picker'), button = picker.querySelector('li button');
        const reason = button.parentElement.querySelector('.tool-unavailable-reason');
        const inside = n => { const r = n.getBoundingClientRect(), p = picker.getBoundingClientRect(); return r.top >= p.top && r.bottom <= p.bottom && r.left >= p.left && r.right <= p.right && r.bottom <= innerHeight && r.right <= innerWidth; };
        return { named: button.textContent.includes('reply_in_conversation'), disabled: button.disabled, reason: reason?.textContent ?? null, rowInside: inside(button.parentElement), reasonInside: reason ? inside(reason) : null,
          described: button.parentElement.getAttribute('aria-describedby') === reason?.id };
      })()`);
      const replyOk = reply.named && reply.rowInside && reply.disabled && reply.reason === replyReason && reply.reasonInside && reply.described;
      if (!replyOk) throw Error(`Reply tool is offered without its visible reason: ${JSON.stringify(reply)}`);
      const replyImage = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${size}-reply-tool.png`), Buffer.from(replyImage.data, 'base64'));
      await evaluate(`document.querySelector('button[aria-label="Close picker"]').click()`);
    }
    if (name === 'designer-reply-saved') {
      // Saved declaration states; the alert names the tool, whatever shared wording the app uses.
      const chip = () => evaluate(`[...document.querySelectorAll('.tool-pill')].some(pill => pill.querySelector('code')?.textContent === 'reply_in_conversation' && pill.getClientRects().length)`);
      const warned = () => evaluate(`[...document.querySelectorAll('[role="alert"]')].some(alert => alert.textContent.includes('reply_in_conversation') && alert.getClientRects().length)`);
      const settleShot = async suffix => {
        await evaluate('document.querySelector(".agent-tool-row").scrollIntoView({ block: "center" }); document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
        const image = await call('Page.captureScreenshot', { format: 'png' });
        await writeFile(join(output, `${theme}-${size}-${suffix}.png`), Buffer.from(image.data, 'base64'));
      };
      const expectSaved = async (present, label) => {
        await until(async () => await chip() === present, `${label} chip`);
        if (await warned() !== present) throw Error(`${label}: saved reply warning must be ${present}`);
      };
      await expectSaved(true, 'preserved');
      await settleShot('saved-reply-tool');
      await evaluate(`document.querySelector('button[aria-label="Remove Reply in a conversation"]').click()`);
      await expectSaved(false, 'removed');
      await settleShot('saved-reply-tool-removed');
      // Restore through File view, the existing path that works with the tool disabled in the picker.
      await clickAction('File');
      await until(() => evaluate(`!!document.querySelector('#agent-file')`), 'File editor');
      await evaluate(`(() => { const file = document.querySelector('#agent-file'); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(file, ${JSON.stringify(replySavedAgent)}); file.dispatchEvent(new Event('input', { bubbles: true })); file.blur(); })()`);
      await clickAction('Agent');
      await expectSaved(true, 'restored');
      await settleShot('saved-reply-tool-restored');
    }
    if (['ai-connected', 'local-completed'].includes(name)) {
      const complete = await evaluate(`({ heading: document.querySelector('h1').textContent, card: document.querySelector('.setup-progress .steps-head h2')?.textContent, pill: document.querySelector('.setup-progress .pill')?.textContent, rows: document.querySelectorAll('[aria-label="Setup steps"] > li').length, text: document.body.innerText, model: document.querySelector('.setup-model').innerText })`);
      if (complete.heading !== readyCardTitle || complete.card !== readyCardTitle || complete.pill !== 'Ready' || /step \d+ of [45]/i.test(complete.text) || complete.rows !== 4 || !complete.text.includes('Stop the local cluster') || complete.model.includes('GitHub CLI must be installed')) throw Error(`Completed local page is not collapsed: ${JSON.stringify(complete)}`);
      if (!complete.model.includes(fixtureAccount) || !complete.model.includes('gpt-5.6-terra')) throw Error(`Connected AI must show the verified account and fixed model: ${JSON.stringify(complete)}`);
    }
    if (name === 'ai-idle') {
      const idle = await evaluate(`({ model: [...document.querySelectorAll('.setup-model code')].map(code => code.textContent), text: document.querySelector('.setup-model').innerText, pickers: document.querySelectorAll('.setup-model select, .setup-model [role=combobox]').length })`);
      if (!idle.model.includes('gpt-5.6-terra') || idle.pickers || /Azure|signed in as|Saved GitHub sign-in/i.test(idle.text)) throw Error(`AI choice must show the fixed model without a picker or an unverified account: ${JSON.stringify(idle)}`);
    }
    if (name === 'ai-connecting' && await evaluate(`[...document.querySelectorAll('button')].some(button => /^Remove (sign-in|key)$/.test(button.textContent) && button.getClientRects().length)`)) throw Error('Remove is offered while Connecting');
    if (name === 'ai-removal-retry' && !await evaluate(`document.querySelector('h1').textContent === 'This computer' && document.body.innerText.includes("The saved sign-in couldn't be removed") && document.querySelector('#model-title')?.textContent === "Couldn't remove the saved sign-in" && ![...document.querySelectorAll('button')].some(button => button.textContent === 'Remove sign-in' && button.getClientRects().length)`)) throw Error('Removal failure lost its headings or still offers Remove');
    if (name === 'ai-idle' && await evaluate(`document.querySelector('.setup-model').innerText.includes('Remove sign-in')`)) throw Error('Unused sign-in removal is visible before connecting');
    if (await evaluate(`document.body.innerText.includes('Change where agents run')`)) throw Error('Removed change panel is still visible');
    if (name === 'designer-connected' || name === 'designer-remote') {
      const settle = () => evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
      const shoot = async suffix => {
        await settle();
        const image = await call('Page.captureScreenshot', { format: 'png' });
        await writeFile(join(output, `${theme}-${size}-${name}-${suffix}.png`), Buffer.from(image.data, 'base64'));
        await compareDesigner(image.data, theme, size, `${name}-${suffix}`);
      };
      const pressed = () => evaluate(`document.querySelector('.view-switch [aria-pressed="true"]')?.textContent`);
      // Explicit Open (first Designer entry) must land on Agent.
      if (await pressed() !== 'Agent') throw Error(`${name}: Open must show Agent`);
      await evaluate(`document.querySelector('main').scrollTop = 0`);
      const type = await evaluate(`(() => { const f = n => n && getComputedStyle(n); const mono = c => /mono|consolas|courier/i.test(c?.fontFamily ?? ''); const label = document.querySelector('.designer-file label[for$="-model"]'); const span = document.querySelector('.test-name > span'), code = document.querySelector('.test-name > code'); const agent = document.querySelector('.agent-name'); const sel = document.querySelector('.model-select'); return { agent: { family: f(agent)?.fontFamily, mono: mono(f(agent)) }, label: label && { size: f(label).fontSize, weight: f(label).fontWeight, mono: mono(f(label)) }, testTitle: span && { family: f(span).fontFamily, weight: f(span).fontWeight, mono: mono(f(span)) }, testId: code && { family: f(code).fontFamily, mono: mono(f(code)) }, model: sel && { value: sel.value, shown: sel.selectedOptions[0]?.textContent }, defaultRepeated: /Default model: /.test(document.querySelector('.designer-file')?.innerText ?? '') }; })()`);
      await writeFile(join(output, `${theme}-${size}-${name}-typography.json`), JSON.stringify(type, null, 2));
      // The chosen agent name stays mono by design; test titles use UI type; IDs mono; form labels 13px/500.
      if (!type.agent.mono || !type.label || type.label.mono || type.label.size !== '13px' || type.label.weight !== '500') throw Error(`${name}: wrong agent-name/form-label type: ${JSON.stringify(type)}`);
      if (name === 'designer-connected') {
        if (!type.testTitle || type.testTitle.mono || type.testTitle.weight !== '500' || !type.testId?.mono) throw Error(`${name}: test title must be UI font/500 with a mono ID: ${JSON.stringify(type)}`);
        if (type.model?.value !== 'gpt-5.6-terra' || type.model.shown !== 'Workspace default: GPT-5.6 Terra' || type.defaultRepeated) throw Error(`${name}: workspace default needs a friendly label, unchanged value and no repeated default: ${JSON.stringify(type)}`);
      }
      // Header geometry must not move across Agent, Chat and File.
      const header = async () => { await evaluate(`document.querySelector('main').scrollTop = 0`); return evaluate(`(() => { const r = s => { const b = document.querySelector(s)?.getBoundingClientRect(); return b && { x: b.x, y: b.y, w: b.width, h: b.height }; }; return { switch: r('.view-switch'), picker: r('.agent-selector'), send: r('.send-entry'), name: r('.agent-name') }; })()`); };
      const base = await header();
      for (const view of ['Chat', 'File']) {
        await clickAction(view);
        await until(async () => await pressed() === view, `${view} view`);
        const now = await header();
        for (const key of Object.keys(base)) if (!base[key] || !now[key] || ['x', 'y', 'w', 'h'].some(k => Math.abs(base[key][k] - now[key][k]) > 2)) throw Error(`${name}: header ${key} moved in ${view}: ${JSON.stringify([base, now])}`);
        if (view === 'Chat') {
          await until(() => evaluate(`!!document.querySelector('.chat-heading')?.getClientRects().length && !document.body.innerText.includes('Opening your conversation')`), 'Chat snapshot');
          const chat = await evaluate(`(() => { const root = document.querySelector('.chat-heading').parentElement; return { text: root.innerText, reset: [...root.querySelectorAll('button')].find(b => b.textContent === 'New conversation')?.disabled }; })()`);
          await writeFile(join(output, `${theme}-${size}-${name}-chat-state.json`), JSON.stringify(chat, null, 2));
          // Remote is parent-blocked; the idle ready snapshot must not be shown as Ready. Unblocked state is recorded only.
          if (name === 'designer-remote') {
            if (/\bReady\b/.test(chat.text) || chat.reset !== true) throw Error(`Blocked Chat must not say Ready and must disable New conversation: ${JSON.stringify(chat)}`);
            await shoot('chat-blocked');
            // Top-level tab navigation retains the last view.
            await evaluate(`document.querySelector('nav[aria-label="Main"] button[aria-label="Overview"]').click()`);
            await until(() => evaluate(`!document.querySelector('.chat-heading')?.getClientRects().length`), 'Chat hidden while on Overview');
            await evaluate(`document.querySelector('nav[aria-label="Main"] button[aria-label="Designer"]').click()`);
            await until(async () => await pressed() === 'Chat' && await evaluate(`!!document.querySelector('.chat-heading')?.getClientRects().length`), 'tab return keeps Chat');
          }
        }
        if (view === 'File') await shoot('file');
      }
      await clickAction('Agent');
      await until(async () => await pressed() === 'Agent', 'Agent view');
      if (name === 'designer-connected') {
        await clickAction('Send to a cluster');
        await until(() => evaluate(`!!document.querySelector('.send-panel')?.getClientRects().length && document.activeElement === document.querySelector('.send-panel')`), 'opened Send is shown and focused');
        if (await evaluate(`document.querySelector('.send-history details').open`)) throw Error('Opening Send must not open Past Sends');
        await evaluate(`document.querySelector('.send-panel').scrollIntoView({ block: 'start' })`);
        await shoot('send');
        await evaluate(`document.querySelector('.send-history summary').click()`);
        await until(() => evaluate(`document.querySelector('.send-history summary').getAttribute('aria-expanded') === 'true'`), 'Past Sends opens on request');
        await evaluate(`document.querySelector('.send-history').scrollIntoView({ block: 'center' })`);
        await shoot('history');
      } else if (!await evaluate(`!!document.querySelector('.send-panel')?.getClientRects().length`)) throw Error('Returning from Chat lost the retained Send');
    }
    console.log(`Captured native WebView2 action fixture: ${theme}/${size}/${name}`);
  }
  // Overview renderer fixtures: synthetic IPC in an isolated world running the compiled app, shaped like app/src/overview/api.ts.
  // Chat's generated Agent is in the live list on purpose and must never be shown.
  const overviewVariants = runCore ? ['clusters', 'local-only', 'starting'] : [];
  async function captureOverview(theme, size, variant) {
    ({ executionContextId: contextId } = await call('Page.createIsolatedWorld', { frameId: frameTree.frame.id, worldName: `overview-${theme}-${size}-${variant}` }));
    await evaluate(`(() => {
      const variant = ${JSON.stringify(variant)};
      const hex = (c, n) => c.repeat(n);
      const installation = '11111111-1111-4111-8111-111111111111';
      const src = c => ({ agent: hex(c, 64), tests: hex(String.fromCharCode(c.charCodeAt(0) + 1), 64) });
      const now = Date.now();
      const runAgent = (n, c) => ({ name: 'web-reader-test-' + n, namespace: 'default', uid: 'run-uid-' + c });
      const result = (id, c, verdict, passed, total, agent) => ({ installation, documentId: id, source: src(c), namespace: agent.namespace, agentName: agent.name, agentUid: agent.uid, observedAt: now - 3600000, state: 'complete', report: { verdict, passed, total } });
      const ids = ['aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', 'cccccccc-cccc-4ccc-8ccc-cccccccccccc'];
      const failedRun = runAgent(1, 'f'), passedRun = runAgent(2, 'e');
      const file = (id, name, c, description, localResult) => ({ documentId: id, name, state: 'readable', source: src(c), summary: { name, description, model: 'fixture-model', tools: ['web_fetch'] }, reason: null, localResult });
      const files = { state: 'complete', historyState: 'complete', runActive: false,
        files: [file(ids[0], 'checksum-checker', '1', 'Checks a page checksum', result(ids[0], '1', 'failed', 1, 3, failedRun)),
          file(ids[1], 'web-reader', '3', 'Answers from the web page named in the request', result(ids[1], '3', 'passed', 2, 2, passedRun)),
          file(ids[2], 'draft-helper', '5', 'A new agent with no tests run yet', null)],
        runAgents: [failedRun, passedRun].map(a => ({ installation, namespace: a.namespace, agentUid: a.uid })) };
      if (variant === 'clusters') {
        files.files[1].source = files.files[1].localResult.source = src('7'); // Tested locally since Send; old bytes now belong to another file.
        files.files[2] = { ...files.files[2], draft: true, summary: null, reason: 'Add instructions and a test to get started.' };
        files.files.push(file('dddddddd-dddd-4ddd-8ddd-dddddddddddd', 'web-reader-copy', '3', 'Identical to the earlier files, but a different document', null));
      }
      const chatAgent = { name: 'bridge-chat-agent-' + hex('0', 32), namespace: 'default', uid: 'chat-uid-0' };
      const live = { state: 'complete', installation, agents: [failedRun, passedRun, chatAgent], observedAt: now, observation: null, runActive: false, reason: null };
      const remote = { name: 'web-reader', namespace: 'agents', uid: 'remote-uid-1', agentType: 'Declarative', model: 'fixture-model', tools: ['web_fetch'],
        record: { id: '22222222-2222-4222-8222-222222222222', source: src('3'), targetDisplay: 'team-dev', targetUids: { serviceNamespace: 'control', serviceUid: 'svc-uid', deploymentNamespace: 'control', deploymentUid: 'dep-uid', watchedNamespace: 'agents' }, provider: 'team-ai', plannedAgentName: 'web-reader', agentUid: 'remote-uid-1', verdict: 'passed', time: Math.floor(now / 1000) - 7200,
          documentId: ids[1], detailState: 'saved', report: { verdict: 'passed', passed: 2, total: 2 } } };
      const failedRemote = { ...remote, name: 'checksum-checker', uid: 'remote-uid-2', record: { ...remote.record,
        id: '33333333-3333-4333-8333-333333333333', documentId: ids[0], plannedAgentName: 'checksum-checker', agentUid: 'remote-uid-2', verdict: 'failed', report: { verdict: 'failed', passed: 1, total: 3 } } };
      const external = { ...remote, name: 'review-helper', uid: 'external-uid', record: null };
      const places = { historyState: 'complete', unobserved: [], places: variant === 'clusters' ? [{ key: 'fixture-remote', context: 'team-dev', service: 'control / orka', namespace: 'agents', stale: false, observedAt: now - 60000, agents: [remote, failedRemote, external] }] : [] };
      const progress = { phase: 'starting', bytes: null, totalBytes: null, elapsedMs: 12000, remainingMs: null };
      const starting = variant === 'starting';
      localStorage.setItem('bridge.welcomeSeen.v1', '1');
      window.isTauri = true;
      window.__TAURI_INTERNALS__ = { invoke: async command => {
        if (command === 'setup_status') return { selected: 'localVm', localVm: starting ? 'starting' : 'ready', model: 'locked', modelSupported: true, workspaceProgress: starting ? progress : null, localVmError: null };
        if (command === 'overview_files') return files;
        if (command === 'overview_agents') return live;
        if (command === 'overview_places') return places;
        if (command === 'setup_example') return ${JSON.stringify(pair)};
        if (command === 'setup_requirements') return null;
        if (command === 'cluster_places') return { places: [], selectedKey: null, persistenceState: 'complete' };
        if (command === 'engine_call') throw { kind: 'unavailable' };
        throw Error('Unmocked Overview fixture command: ' + command);
      } };
      document.getElementById('root').replaceWith(Object.assign(document.createElement('div'), { id: 'root' }));
    })()`);
    await evaluate(bundle);
    await until(() => evaluate(`!!document.querySelector('button.page-tab[aria-label="Overview"]')`), 'Overview tab');
    await evaluate(`document.querySelector('button.page-tab[aria-label="Overview"]').click()`);
    const text = () => evaluate(`document.querySelector('section.overview:not([hidden])')?.innerText ?? ''`);
    const shoot = async suffix => {
      await evaluate('document.fonts.ready.then(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))');
      const image = await call('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(output, `${theme}-${size}-overview-${variant}${suffix}.png`), Buffer.from(image.data, 'base64'));
      console.log(`Captured native WebView2 Overview fixture: ${theme}/${size}/${variant}${suffix}`);
    };
    const check = async mode => {
      const body = await text();
      const fail = reason => { throw Error(`overview-${variant}/${size}/${mode}: ${reason}: ${body}`); };
      if (variant === 'starting') {
        if (!/Starting the local cluster/.test(body) || /stopped/i.test(body)) fail('startup must say starting, not stopped');
        return;
      }
      for (const label of ['Failed', 'Passed', 'Not tested']) if (!body.includes(label)) fail(`missing ${label} verdict`);
      if (/bridge-chat-agent/.test(body)) fail('generated Chat Agent is visible');
      if (variant === 'clusters') {
        const proof = await evaluate(`(() => {
          const find = key => document.querySelector('[data-card-key="' + key + '"]');
          const describe = card => ({ text: card?.innerText, buttons: [...(card?.querySelectorAll('button, [role="button"]') ?? [])].map(b => b.textContent), group: card?.closest('.overview-group')?.querySelector('h2')?.textContent });
          return { passed: describe(find('fixture-remote/remote-uid-1')), failed: describe(find('fixture-remote/remote-uid-2')), draft: describe(find('local/cccccccc-cccc-4ccc-8ccc-cccccccccccc')), external: describe(find('fixture-remote/external-uid')) };
        })()`);
        if (!proof.passed.text?.includes('Earlier files') || !proof.passed.text.includes('2 of 2 tests on earlier files') || !proof.failed.text?.includes('1 of 3 tests on earlier files') || !proof.failed.text.includes('Failed')) fail('final counts or earlier-file verdicts missing');
        if (mode === 'by-agent' && (proof.passed.group !== 'web-reader' || proof.failed.group !== 'checksum-checker')) fail('document identity lost to another file hash');
        if (proof.draft.buttons.join('|') !== 'Open|Details' || !proof.draft.text?.includes('Draft') || !body.includes('Add instructions and a test to get started.')) fail('blank draft must be Open-only');
        if (proof.external.buttons.join('|') !== 'Details' || !proof.external.text?.includes('No agent file on this computer.')) fail('external Agent must be Details-only');
        if (![proof.passed, proof.failed].every(c => c.buttons.join('|') === 'Open|Run tests|Details')) fail('linked cluster actions missing');
      }
      if (variant === 'local-only' && body.includes('team-dev')) fail('remote cluster should be absent');
      const overflow = await evaluate(`(() => { const main = document.querySelector('main'); return main.scrollWidth - main.clientWidth; })()`);
      if (overflow > 2) fail('page overflows horizontally');
    };
    if (variant === 'starting') {
      await until(async () => /Starting the local cluster/.test(await text()), 'Overview startup note');
      await check('');
      await shoot('');
      return;
    }
    await until(async () => (await text()).includes('Not tested'), 'Overview cards');
    const captureCards = async mode => {
      await check(mode);
      await evaluate(`document.querySelector('main').scrollTo(0, 0)`);
      await shoot('-' + mode);
      if (variant !== 'clusters') return;
      // These are fixed expressions, not data interpolated into executable JavaScript.
      for (const [scene, card] of [
        ['earlier-pass', `document.querySelector('[data-card-key="fixture-remote/remote-uid-1"]')`],
        ['earlier-failed', `document.querySelector('[data-card-key="fixture-remote/remote-uid-2"]')`],
        ['draft', `document.querySelector('[data-card-key="local/cccccccc-cccc-4ccc-8ccc-cccccccccccc"]')`],
      ]) {
        await evaluate(`${card}.scrollIntoView({ block: 'center' })`);
        const visible = await evaluate(`(() => { const r = ${card}.getBoundingClientRect(); return r.top >= 0 && r.bottom <= innerHeight; })()`);
        if (!visible) throw Error('Overview evidence card is outside the viewport: ' + scene);
        await shoot('-' + mode + '-' + scene);
      }
    };
    await captureCards('by-cluster');
    await clickAction('By agent');
    await until(() => evaluate(`document.querySelector('.view-switch button:nth-child(2)')?.getAttribute('aria-pressed') === 'true'`), 'By agent view');
    await captureCards('by-agent');
  }
  const notes = [];
  for (const theme of ['light', 'dark']) {
    await call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    for (const fixture of setupFixtures) await captureSetup(theme, 'wide', fixture);
    for (const name of actionFixtures) await captureActions(theme, 'wide', name);
    for (const variant of overviewVariants) await captureOverview(theme, 'wide', variant);
  }
  // Request CSS viewport dimensions, converting once for the real native client area.
  // Verify both Win32 physical pixels and the embedded WebView; never emulate or assume a resize.
  async function resizeTo(target, label) {
  const viewport = async () => evaluate('({ width: innerWidth, height: innerHeight, dpr: devicePixelRatio })');
  ({ executionContextId: contextId } = await call('Page.createIsolatedWorld', { frameId: frameTree.frame.id, worldName: 'viewport-probe' }));
  const ratio = (await viewport()).dpr;
  const physical = { width: Math.round(target.width * ratio), height: Math.round(target.height * ratio) };
  const resizeScript = `
$ErrorActionPreference = 'Stop'
Add-Type @'
using System; using System.Runtime.InteropServices;
public struct WinRect { public int L, T, R, B; }
public static class WinSize {
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out WinRect r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out WinRect r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int h2, uint flags);
}
'@
# PMv2 keeps these Win32 coordinates physical, independent of PowerShell's default awareness.
if ([WinSize]::SetThreadDpiAwarenessContext([IntPtr](-4)) -eq [IntPtr]::Zero) { throw 'Cannot select physical-pixel coordinates' }
$h = (Get-Process -Id ${child.pid}).MainWindowHandle
if ($h -eq [IntPtr]::Zero) { throw 'No native main window handle' }
for ($i = 0; $i -lt 4; $i++) {
  $w = New-Object WinRect; $c = New-Object WinRect
  [void][WinSize]::GetWindowRect($h, [ref]$w); [void][WinSize]::GetClientRect($h, [ref]$c)
  $dw = ${physical.width} - ($c.R - $c.L); $dh = ${physical.height} - ($c.B - $c.T)
  if ($dw -eq 0 -and $dh -eq 0) { break }
  [void][WinSize]::SetWindowPos($h, [IntPtr]::Zero, $w.L, $w.T, ($w.R - $w.L) + $dw, ($w.B - $w.T) + $dh, 0x0014)
  Start-Sleep -Milliseconds 300
}
$c = New-Object WinRect; [void][WinSize]::GetClientRect($h, [ref]$c)
"client=$($c.R - $c.L)x$($c.B - $c.T)"`;
  // EncodedCommand avoids any argument-quoting ambiguity for the embedded C#.
  const resized = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-EncodedCommand', Buffer.from(resizeScript, 'utf16le').toString('base64')], { encoding: 'utf8', timeout: 20000 });
  let mode = 'native-window-resize';
  await delay(500);
  let reported = await viewport();
  const close = (a, b) => Math.abs(a - b) <= 2;
  const client = /client=(\d+)x(\d+)/.exec(resized.stdout ?? '');
  if (resized.status !== 0 || reported.width !== target.width || reported.height !== target.height || !client
      || !close(Number(client[1]), reported.width * reported.dpr) || !close(Number(client[2]), reported.height * reported.dpr)) {
    throw Error(`Native client resize to ${target.width}x${target.height} CSS px (${physical.width}x${physical.height} physical px) unverified (exit ${resized.status}): ${JSON.stringify(reported)} ${resized.stdout ?? ''} ${resized.stderr ?? ''}`);
  }
  notes.push(`${label} captures: ${mode}. Requested content area ${target.width}x${target.height} CSS px (${physical.width}x${physical.height} physical px); WebView reported ${reported.width}x${reported.height} CSS px at devicePixelRatio ${reported.dpr}. ${resized.stdout.trim()}`);
  }
  await resizeTo({ width: 540, height: 640 }, 'Compact');
  for (const theme of ['light', 'dark']) {
    await call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    for (const fixture of setupFixtures) await captureSetup(theme, 'compact', fixture);
    for (const name of actionFixtures) await captureActions(theme, 'compact', name);
    for (const variant of overviewVariants) await captureOverview(theme, 'compact', variant);
  }
  // Preserve the explicitly named Overview minimum captures alongside the compact suite.
  await resizeTo({ width: 540, height: 640 }, 'Minimum');
  for (const theme of ['light', 'dark']) {
    await call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    for (const variant of overviewVariants) await captureOverview(theme, 'minimum', variant);
  }
  for (const theme of ['light', 'dark']) {
    await call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    if (runCore) for (const name of ['designer-connected', 'designer-running', 'designer-failed', 'designer-passed', 'designer-stale']) await captureActions(theme, 'minimum', name);
  }
  await writeFile(join(output, 'compact-window.txt'), `Capture scope: ${captureScope}\n` + notes.join('\n') + '\n');
  await writeFile(join(output, 'README.txt'), `Capture scope: ${captureScope}. CI keeps core and chat-agent-clusters artifacts separately; both are required for the complete matrix.\n` + 'Injected renderer states in the final-head embedded Windows app WebView2. Setup-progress states (setup-*), Places and local size states (places-*, local-preflight*, including ready, unknown, blocked and cluster-selected choices), Welcome (welcome*), completed local/Stop pages (local-completed, local-stopped) and action-driven returning-workspace lifecycle states (local-workspace-*, compared with the approved workspace reference), removal confirmation/retry (ai-removal-*), earlier-installation AI reset states beside approved frames 1g/1h (ai-reset-*), Add-a-place stages (cluster-*) and setup actions/first-test/Designer states use synthetic IPC. Local Cancel states (local-cancel-*) exercise renderer controls with synthetic cancellation/worker results, not native cancellation proof. First-test outcomes (first-test-passed, -unknown, -lost-reply) each reload the page and own one synthetic automatic attempt with an identical source; only the acknowledged passed submission may show the first-pass story. Setup Meet/queued, AI idle/key/connected and first-pass captures have *-comparison.png images beside cropped approved light frames 1c, 1d and 1e (dark captures use the light frame). ai-key never types a key. Layout JSON records setup stacks, form widths, row alignment, card insets, Designer priority, loading/open-error fallback spacing, fixed wide tabs/host gap, compact subtitles/label, quiet padding and themed thin scrollbars. Overview states (overview-*, including 540x640 minimum) use synthetic overview_* IPC. Chat empty/pending/running/answer and waitingForSession Succeeded/null or known-answer, New agent form/blank-created, Clusters list, context picker/checking/failure are captured at wide and true 540x640 client sizes in both themes with comparisons. Chat/New agent compare the Designer design system, not a dedicated target frame; cluster states compare the approved cluster cards (light reference for both themes). compact-window.txt records verified native client resizing; an unverified resize fails rather than falling back to viewport emulation. Not native IPC, VM, credential or download-path verification. Download is reserved and unreachable from current native startup. Themes are WebView2 media emulation; system preferences are unchanged.\n');
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
