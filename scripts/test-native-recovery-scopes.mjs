// Exercise the real driver's argument parsing and fixture rosters without launching Windows.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import vm from 'node:vm';

const source = (await readFile(new URL('./native-recovery-windows.mjs', import.meta.url), 'utf8')).replaceAll('\r\n', '\n');
const prefix = source.slice(0, source.indexOf('for (const root of')).replace(/^import .*;\n/gm, '');
function selection(scope, ...extra) {
  const argv = ['node', 'driver', 'app.exe', 'captures', ...(scope === undefined ? [] : [scope]), ...extra];
  return vm.runInNewContext(`${prefix}\n({ executable, output, scope: captureScope, runCore, runUi, runUpgrade })`, {
    process: { platform: 'win32', env: { GITHUB_ACTIONS: 'true' }, argv }, resolve,
  });
}
assert.throws(() => selection('typo'), /capture scope/i, 'unknown scope must not silently run the full suite');
assert.equal(selection().scope, 'all');
for (const [scope, expected] of [
  ['all', [true, true, true]], ['core', [true, false, false]],
  ['chat-agent-clusters', [false, true, false]], ['workspace-upgrade', [false, false, true]],
]) {
  const chosen = selection(scope);
  assert.deepEqual([chosen.runCore, chosen.runUi, chosen.runUpgrade], expected, scope);
  assert.equal(chosen.output, resolve('captures'));
}
for (const args of [['typo'], ['--scope'], ['--scope', 'core'], ['core', 'core'], ['workspace-upgrade', 'all']]) {
  assert.throws(() => selection(...args), /all\|core\|chat-agent-clusters\|workspace-upgrade/, `invalid arguments must list finite positional scopes: ${args}`);
}
assert.equal(selection('core').runUi, false);
assert.equal(selection('chat-agent-clusters').runCore, false);
assert.equal(selection('chat-agent-clusters').output, resolve('captures'), 'scope must not become a path');

function roster(name, chosen) {
  const expression = source.match(new RegExp(`const ${name} = ([\\s\\S]*?);\\n`))?.[1];
  assert.ok(expression, `missing ${name}`);
  return Function('runCore', 'runUi', 'transfer', 'startup', `return (${expression});`)(chosen.runCore, chosen.runUi, {}, () => ({}));
}
function actions(chosen) {
  const names = roster('actionFixtures', chosen);
  const append = source.match(/^  .*actionFixtures\.push\([^\n]+$/m)?.[0];
  assert.ok(append);
  Function('runUi', 'actionFixtures', append)(chosen.runUi, names);
  return names;
}
const all = actions(selection()), core = actions(selection('core')), ui = actions(selection('chat-agent-clusters'));
assert.equal(all.length, 72);
assert.equal(core.length, 47);
assert.equal(ui.length, 25);
assert.deepEqual(actions(selection('workspace-upgrade')), [], 'upgrade scope must not repeat any of the 47+25 actions');
assert.ok(core.includes('designer-reply-saved'), 'saved reply captures belong to the core scope');
assert.ok(!ui.includes('designer-reply-saved'), 'UI scope must not repeat saved reply captures');
assert.equal(new Set([...core, ...ui]).size, all.length, 'scopes must partition, not repeat or omit cases');
assert.deepEqual([...core, ...ui], all);
assert.deepEqual(ui, ['chat-empty', 'chat-pending', 'chat-running', 'chat-answer', 'chat-session-null', 'chat-session-answer', 'chat-two-versions', 'chat-long', 'send-ready', 'send-blocked', 'send-sending', 'send-passed', 'send-failed', 'send-unsupported', 'send-earlier-target', 'new-agent-form', 'new-agent-created', 'cluster-picker-open', 'cluster-checking', 'cluster-failure', 'cluster-connected', 'cluster-account-picker-open', 'cluster-account-allowed', 'cluster-account-forbidden', 'designer-no-destination']);
const chatStatus = source.match(/if \(command === 'agent_chat_status'\) \{([\s\S]*?return \{ documentId,[^\n]+;)\n        \}/)?.[1];
assert.ok(chatStatus, 'bounded Chat snapshot producer is missing');
for (const name of ['chat-pending', 'chat-running', 'chat-answer', 'chat-session-null', 'chat-session-answer']) {
  const snapshot = Function('name', 'documentId', chatStatus)(name, 'fixture-document');
  const turn = snapshot.versions[0].turns[0];
  const confirming = name.startsWith('chat-session-');
  assert.equal(snapshot.state, confirming ? 'waitingForSession' : name === 'chat-answer' ? 'ready' : 'working', name);
  assert.equal(turn.phase, confirming || name === 'chat-answer' ? 'Succeeded' : name === 'chat-pending' ? 'Pending' : 'Running', name);
  assert.equal(turn.answer, ['chat-answer', 'chat-session-answer'].includes(name) ? 'The page says fixture-checksum.' : null, name);
  assert.equal(turn.contextRetained, name === 'chat-answer', name);
  assert.equal(snapshot.busy, name !== 'chat-answer', name);
  assert.equal(snapshot.canSend, name === 'chat-answer', name);
}
for (const name of ['chat-answer', 'chat-long']) {
  const snapshot = Function('name', 'documentId', chatStatus)(name, 'fixture-document');
  for (const turn of snapshot.versions[0].turns) for (const tool of turn.tools) {
    assert.equal(typeof tool.callId, 'string', 'Chat tool fixtures must pass the production snapshot boundary');
  }
}
const twoVersions = Function('name', 'documentId', chatStatus)('chat-two-versions', 'fixture-document');
assert.equal(twoVersions.versions.length, 2);
assert.equal(twoVersions.versions[0].turns[0].message, 'Keep this earlier conversation.');
assert.equal(twoVersions.versions[1].turns[0].message, 'What did the page say?');
for (const name of ['fixtures', 'setupFixtures']) {
  assert.equal(roster(name, selection('chat-agent-clusters')).length, 0, `UI scope must not repeat ${name}`);
  assert.ok(roster(name, selection('core')).length > 0);
  assert.equal(roster(name, selection('workspace-upgrade')).length, 0, `upgrade scope must not repeat ${name}`);
}
const overviewAll = roster('overviewVariants', selection());
const overviewCore = roster('overviewVariants', selection('core'));
const overviewUi = roster('overviewVariants', selection('chat-agent-clusters'));
assert.deepEqual(overviewAll, ['clusters', 'local-only', 'starting']);
assert.deepEqual(overviewCore, [], 'Overview must leave the constrained Core scope');
assert.deepEqual(roster('overviewVariants', selection('workspace-upgrade')), [], 'upgrade scope must not repeat Overview');
assert.deepEqual(overviewUi, overviewAll, 'UI must own the complete Overview matrix');
assert.deepEqual([...overviewCore, ...overviewUi], overviewAll, 'split Overview coverage must match all');
assert.equal(new Set([...overviewCore, ...overviewUi]).size, overviewAll.length, 'Overview must not repeat between scopes');
assert.match(source, /if \(runCore\) for \(const name of \['designer-connected'/, 'UI scope must not repeat minimum Designer cases');
// Execute the capture gate with accepted and violating observations. This tests
// the gate, not native rendering; CI supplies the actual DOM and window measurements.
const sendGate = source.slice(source.indexOf('  function assertSendPresentation('), source.indexOf('  async function captureSendScene('));
assert.ok(sendGate.includes('Unsupported cancellation'), 'Send capture acceptance gate is missing');
const checkSend = Function(`${sendGate}; return assertSendPresentation;`)();
const sendProbe = { header: [{ label: 'Fix in Designer', primary: true }, { label: 'Run cluster tests again', primary: false }],
  buttons: ['Fix in Designer', 'Run cluster tests again'], title: 'Captured checksum from the page', id: 'page-checksum',
  heading: 'To fixture-selected', summary: 'web-reader with 1 test', checks: 'Ready to send: 1 check passed Show checks', checksOpen: false,
  inlineChecks: [], changeDisabled: false, text: 'fixture-captured', detailsOpen: false, extras: [], width: 540, scrollWidth: 540 };
checkSend('send-failed', sendProbe, false);
const unsupported = { ...sendProbe, header: [{ label: 'Check cluster status', primary: true }], buttons: ['Check cluster status'] };
checkSend('send-unsupported', unsupported, false);
checkSend('send-earlier-target', { ...sendProbe, header: [{ label: 'Send and run tests', primary: true }, { label: 'See in Overview', primary: false }], text: 'Earlier result on fixture-captured To fixture-selected' }, false);
checkSend('send-failed', { ...sendProbe, detailsOpen: true, extras: ['Check again', 'Send as a separate agent'] }, true);
for (const [name, header] of [
  ['send-ready', [{ label: 'Send and run tests', primary: true }]],
  ['send-blocked', [{ label: 'Check again', primary: true }]],
  ['send-sending', [{ label: 'Cancel tests', primary: false }]],
  ['send-passed', [{ label: 'See in Overview', primary: true }, { label: 'Run cluster tests again', primary: false }]],
]) {
  const probe = { ...sendProbe, header, title: name === 'send-passed' ? sendProbe.title : '', id: name === 'send-passed' ? sendProbe.id : '',
    checks: ['send-blocked', 'send-sending'].includes(name) ? '1 check passed Show checks' : sendProbe.checks,
    inlineChecks: name === 'send-blocked' ? ['blocked'] : [], changeDisabled: name === 'send-sending' };
  checkSend(name, probe, false);
  assert.throws(() => checkSend(name, { ...probe, checksOpen: true }, false));
  if (name === 'send-sending') {
    assert.throws(() => checkSend(name, { ...probe, changeDisabled: false }, false));
    assert.throws(() => checkSend(name, { ...probe, checks: sendProbe.checks }, false), 'running Send must not advertise readiness');
  }
  if (name === 'send-blocked') assert.throws(() => checkSend(name, { ...probe, inlineChecks: ['allowed', 'blocked'] }, false));
}
for (const mutation of [{ heading: 'Send your agent' }, { summary: '' }, { checks: 'Ready to send: 2 checks passed' }]) {
  assert.throws(() => checkSend('send-failed', { ...sendProbe, ...mutation }, false));
}
const documentScrollGuard = source.split('\n').find(line => line.includes("fail('document overflows"));
assert.ok(documentScrollGuard, 'screen-reader-only content must not escape the page scroll');
const checkDocumentScroll = Function('layout', 'name', 'fail', documentScrollGuard);
assert.doesNotThrow(() => checkDocumentScroll({ documentScrollHeight: 900, height: 900 }, 'chat-long', assert.fail));
assert.doesNotThrow(() => checkDocumentScroll({ documentScrollHeight: 719, height: 640 }, 'ai-key', assert.fail));
assert.throws(() => checkDocumentScroll({ documentScrollHeight: 2105, height: 900, designer: { send: {} } }, 'send-ready', assert.fail));
assert.throws(() => checkDocumentScroll({ documentScrollHeight: 2105, height: 900 }, 'chat-long', assert.fail));
assert.doesNotThrow(() => checkDocumentScroll({ documentScrollHeight: 640, height: 640 }, 'upgrade-consumed-restore-confirm', assert.fail));
assert.throws(() => checkDocumentScroll({ documentScrollHeight: 720, height: 640 }, 'upgrade-consumed-restore-confirm', assert.fail));
// The Chat subtitle can resize the heading, but not reorder or independently move its controls.
const headerGate = source.match(/const now = await header\(\);\n([\s\S]*?)\n        if \(view === 'Chat'\)/)?.[1];
assert.ok(headerGate);
const verifyDesignerHeader = Function('base', 'now', 'view', 'name', headerGate);
const headerBase = Object.fromEntries(['switch', 'picker', 'send', 'name'].map((key, i) => [key, { x: i * 100, y: 80, w: 80, h: 32 }]));
const chatHeader = Object.fromEntries(Object.entries(headerBase).map(([key, rect]) => [key, { ...rect, y: key === 'name' ? 75 : 88, w: key === 'name' ? 200 : rect.w }]));
assert.doesNotThrow(() => verifyDesignerHeader(headerBase, chatHeader, 'Chat', 'fixture'));
assert.throws(() => verifyDesignerHeader(headerBase, chatHeader, 'File', 'fixture'));
assert.throws(() => verifyDesignerHeader(headerBase, { ...chatHeader, picker: { ...chatHeader.picker, x: 120 } }, 'Chat', 'fixture'));
assert.throws(() => verifyDesignerHeader(headerBase, { ...chatHeader, picker: { ...chatHeader.picker, y: 100 } }, 'Chat', 'fixture'));
// Exercise the actual Chat gate, including composer geometry while reading earlier turns.
const chatGate = source.slice(source.indexOf('  function assertChatPresentation('), source.indexOf('  async function captureActions('));
assert.ok(chatGate.includes('earlier'), 'Chat needs a reader-position geometry gate');
const checkChat = Function(`${chatGate}; return assertChatPresentation;`)();
const chatProbe = { card: false, headingHidden: true, label: 'Chat on this computer', heading: 'Chat on this computer',
  subtitle: "Chat on this computer. Chat doesn't change test results.", text: '', status: '', numbers: [], controls: [],
  hint: 'Enter sends · Shift+Enter for a new line', sendLabel: 'Send message', sendText: 'Send', fieldLabel: 'Message', tools: [], toolDisclosure: false,
  composer: { top: 480, bottom: 600, left: 24, right: 516 }, field: { top: 480, bottom: 528 }, send: { top: 496, bottom: 528 },
  viewport: { top: 60, bottom: 640, left: 0, right: 540 }, nestedScroll: false, earlier: true, long: true, scrollTop: 140, scrollMax: 1800 };
checkChat('chat-empty', chatProbe);
checkChat('chat-answer', { ...chatProbe, tools: ['web_fetch · Succeeded'] });
checkChat('chat-long', chatProbe);
for (const mutation of [{ card: true }, { headingHidden: false }, { status: 'Ready for your message.' }, { hint: 'Enter to send' },
  { controls: ['New conversation'] }, { text: 'Ask your agent a question to start a conversation.' },
  { composer: { ...chatProbe.composer, bottom: 650 } }, { nestedScroll: true }]) assert.throws(() => checkChat('chat-empty', { ...chatProbe, ...mutation }));
assert.throws(() => checkChat('chat-answer', { ...chatProbe, tools: [] }));
assert.throws(() => checkChat('chat-answer', { ...chatProbe, tools: ['web_fetch'], status: 'Ready for your message.' }));
assert.throws(() => checkChat('chat-long', { ...chatProbe, earlier: false }));
assert.throws(() => checkChat('chat-long', { ...chatProbe, scrollTop: 1790 }));
for (const mutation of [
  { header: [...sendProbe.header, { label: 'Send as a separate agent', primary: false }] },
  { header: sendProbe.header.map(button => ({ ...button, primary: !button.primary })) },
  { title: '' }, { id: '' }, { scrollWidth: 550 },
]) assert.throws(() => checkSend('send-failed', { ...sendProbe, ...mutation }, false));
assert.throws(() => checkSend('send-unsupported', { ...unsupported, buttons: ['Check cluster status', 'Retry cancellation'] }, false));
assert.throws(() => checkSend('send-earlier-target', { ...sendProbe, text: 'fixture-captured fixture-selected' }, false));
assert.throws(() => checkSend('send-failed', { ...sendProbe, detailsOpen: true, extras: ['Check again'] }, true));
// Exercise the queued-state predicate with the rendered summary, including its workspace wording.
const queuedGuard = source.split('\n').find(line => line.includes('if (!queued.status'));
assert.ok(queuedGuard);
const checkQueued = Function('queued', 'name', queuedGuard);
const queuedState = { status: true, inChoice: false, change: true, cancel: false, text: 'GitHub Copilot, connects when the workspace is ready' };
assert.doesNotThrow(() => checkQueued(queuedState, 'setup-transfer-queued'));
assert.throws(() => checkQueued({ ...queuedState, cancel: true }, 'setup-transfer-queued'));
const welcomeGuard = source.split('\n').find(line => line.includes('if (!welcome.primary'));
assert.ok(welcomeGuard);
const checkWelcome = Function('welcome', welcomeGuard);
const welcomeState = { primary: true, expert: true, text: "Send it to your team's cluster (preview)." };
assert.doesNotThrow(() => checkWelcome(welcomeState));
assert.throws(() => checkWelcome({ ...welcomeState, primary: false }));
// Exercise the real layout gate with measurements, without pretending to render a native window.
const layoutGate = source.slice(source.indexOf('  async function checkLayout('), source.indexOf('  // Setup-progress states:'));
assert.ok(layoutGate.includes('wide tabs must stay 180px'));
const wide = { width: 1280, scrollWidth: 1000, clientWidth: 1000, scrollHeight: 700, clientHeight: 700, forms: [], rows: [], checks: [],
  tabs: [0, 1, 2].map(i => ({ width: 180, grow: '0', right: 180 * (i + 1) })),
  host: { x: 556 }, hostLabel: { width: 100 }, subtitles: 3, quiet: [{ start: '0px', end: '0px' }],
  scrollbar: { width: 'thin', color: 'rgb(90, 90, 90) rgb(240, 240, 240)' }, theme: { page: 'rgb(255, 255, 255)', ink: 'rgb(11, 11, 12)' }, fallback: {} };
async function gate(layout) {
  return Function('evaluate', 'writeFile', 'join', 'output', `${layoutGate}; return checkLayout('light', 'setup-fit', 'probe');`)(
    async () => layout, async () => {}, () => '', '');
}
await gate(wide);
await gate({ ...wide, width: 540, subtitles: 0 });
for (const mutation of [
  { tabs: wide.tabs.map(tab => ({ ...tab, width: 181.5 })) },
  { tabs: wide.tabs.map(tab => ({ ...tab, grow: '1' })) }, { host: { x: 555 } },
  { width: 540 }, { width: 540, subtitles: 0, hostLabel: { width: 161 } },
  { quiet: [{ start: '4px', end: '0px' }] }, { scrollbar: { width: 'auto', color: 'auto' } },
  { theme: { page: 'rgb(242, 241, 237)', ink: 'rgb(27, 32, 38)' } },
  { scrollHeight: 703 },
]) await assert.rejects(() => gate({ ...wide, ...mutation }));
// The compact Change image must be framed before requesting its screenshot.
const changeCapture = source.match(/if \(name\.startsWith\('ai-removal-'\)\) \{([\s\S]*?)\n      await clickAction\('Remove saved sign-in'\);/)?.[1];
assert.ok(changeCapture);
async function capturePanel(canReveal, fixture = 'ai-removal-confirmation') {
  let rect = { top: 780, bottom: 860, left: 24, right: 516, width: 492, height: 80 };
  const viewport = { top: 60, bottom: 640, left: 0, right: 540 };
  let captured = false;
  const document = { querySelector: selector => selector === 'main' ? { getBoundingClientRect: () => viewport } : {
    scrollIntoView() { if (canReveal) rect = { ...rect, top: 300, bottom: 380 }; }, getBoundingClientRect: () => rect,
  } };
  await Function('clickAction', 'until', 'evaluate', 'call', 'writeFile', 'join', 'output', 'theme', 'size', 'name', `return (async () => { ${changeCapture} })()`)(
    async () => {}, async work => assert.equal(await work(), true, 'Change panel must fit before capture'),
    async script => vm.runInNewContext(script, { document }),
    async () => { assert.ok(rect.top >= viewport.top && rect.bottom <= viewport.bottom, 'screenshot requested with Change panel offscreen'); captured = true; return { data: '' }; },
    async () => {}, () => '', '', 'light', 'compact', fixture);
  return captured;
}
assert.equal(await capturePanel(true), true);
await assert.rejects(() => capturePanel(false), /Change panel must fit/);
assert.equal(await capturePanel(true, 'ai-removal-retry'), false, 'retry fixture must not overwrite the identical pre-removal panel image');
const workflow = (await readFile(new URL('../.github/workflows/checks.yml', import.meta.url), 'utf8')).replaceAll('\r\n', '\n');
for (const [scope, directory] of [['core', 'native-recovery'], ['chat-agent-clusters', 'native-chat-agent-clusters'], ['workspace-upgrade', 'native-workspace-upgrade']]) {
  const command = `node ../scripts/native-recovery-windows.mjs src-tauri/target/debug/orka-bridge.exe windows-artifact/${directory} ${scope}`;
  assert.ok(workflow.includes(command), `CI must run ${scope} into its own artifact directory`);
  const before = workflow.slice(0, workflow.indexOf(command));
  const step = before.slice(before.lastIndexOf('      - name:'));
  assert.match(step, /timeout-minutes: 3\n/, 'each scope keeps the existing three-minute ceiling');
}
assert.ok(workflow.includes('node ../scripts/test-native-recovery-scopes.mjs'));
// Effective native grants are checked through Tauri's generated runtime
// authority in tests/config.rs, not by where Rust happens to spell the names.
// Saved-only header counts apply before Overview is visited, not after its deliberate visit.
const hiddenHeader = source.split('\n').find(line => line.includes("'hidden saved-file count'"));
assert.ok(hiddenHeader);
const checkHeader = Function('noDestination', 'until', 'evaluate', `return (async () => { ${hiddenHeader} })()`);
let headerReads = 0;
await checkHeader(true, async () => { headerReads++; }, () => true);
assert.equal(headerReads, 0, 'visited Overview must not be required to keep its pre-visit saved-only label');
await checkHeader(false, async () => { headerReads++; }, () => true);
assert.equal(headerReads, 1, 'other Designer cases retain the hidden saved-only count gate');
// Execute the driver's actual isolated-world IPC setup, not a second copy of its payloads.
const captureActions = source.slice(source.indexOf('  async function captureActions('), source.indexOf('  // Overview renderer fixtures:'));
for (const name of ['cluster-account-picker-open', 'cluster-account-allowed', 'cluster-account-forbidden', 'designer-no-destination', 'send-ready', 'send-blocked', 'send-sending', 'send-passed', 'send-failed', 'send-unsupported', 'send-earlier-target', 'chat-answer', 'chat-long']) {
  let injection;
  const stop = new Error('injection captured');
  const capture = vm.runInNewContext(`(${captureActions})`, {
    pair: { agent: '', tests: '' }, replySavedAgent: '', GiB: 1024 ** 3, fixtureAccount: 'synthetic-user',
    call: async () => ({ frameTree: { frame: { id: 'fixture' } }, executionContextId: 1 }),
    evaluate: async script => { injection = script; throw stop; },
  });
  await assert.rejects(capture('light', 'wide', name), error => error === stop);
  const window = {};
  vm.runInNewContext(injection, { window, localStorage: { setItem() {}, removeItem() {} }, document: { getElementById: () => ({ replaceWith() {} }), createElement: () => ({}) } });
  const invoke = window.__TAURI_INTERNALS__.invoke;
  if (name === 'designer-no-destination') {
    assert.equal((await invoke('setup_status')).selected, null);
    await assert.rejects(invoke('workspace_catalog'), /must not request a catalog/);
    continue;
  }
  if (name === 'chat-answer' || name === 'chat-long') {
    const chat = await invoke('agent_chat_status');
    assert.equal(chat.versions.length, 1);
    assert.ok(chat.versions[0].turns[0].tools.some(tool => tool.name === 'web_fetch'));
    if (name === 'chat-long') assert.ok(chat.versions[0].turns.length >= 16, 'long transcript must exceed both native viewport heights');
    continue;
  }
  const [target] = await invoke('cluster_discover');
  if (name.startsWith('send-')) {
    const { documentId, view, busy } = await invoke('send_status');
    const preflight = await invoke('send_preflight');
    const opened = await invoke('agent_open');
    assert.equal(busy, null);
    assert.equal(preflight.rows.filter(row => row.status === 'allowed').length, 1);
    assert.ok(preflight.rows.some(row => row.status === 'unverified'), 'advisory rows must not inflate the passed count');
    if (name === 'send-ready' || name === 'send-blocked') {
      assert.equal(view, null);
      assert.equal(preflight.canSend, name === 'send-ready');
      assert.equal(preflight.rows.some(row => row.status === 'blocked'), name === 'send-blocked');
      continue;
    }
    if (name === 'send-sending') {
      assert.equal(view.state, 'running');
      assert.equal(view.stage, 'observe');
      assert.equal(view.report, null);
      continue;
    }
    if (name === 'send-passed') assert.equal(view.report.verdict, 'passed');
    assert.equal(view.documentId, documentId);
    assert.equal(view.report.cases[0].title, 'Captured checksum from the page');
    assert.equal(view.report.cases[0].id, 'page-checksum');
    assert.notEqual(view.report.cases[0].title, 'Copies the checksum from the page', 'captured title must not be current editor copy');
    assert.equal(preflight.targetId, target.id);
    assert.equal(preflight.source.agent, opened.source.agent);
    assert.equal(preflight.source.tests, opened.source.tests);
    assert.equal(preflight.canSeparate, true);
    assert.equal(view.cancellation, name === 'send-unsupported' ? 'unsupported' : null);
    if (name === 'send-unsupported') assert.equal(view.report.cases[0].answer, 'The page says fixture-checksum.');
    if (name === 'send-earlier-target') {
      assert.notEqual(view.targetId, target.id);
      assert.equal(view.targetName, 'fixture-captured');
      assert.equal(target.context_name, 'fixture-selected');
      assert.notEqual(view.source.tests, preflight.source.tests);
    }
    continue;
  }
  const args = { targetId: target.id, saName: null };
  assert.equal((await invoke('cluster_use', args)).status, 'needServiceAccount');
  if (name === 'cluster-account-forbidden') {
    await assert.rejects(invoke('cluster_service_accounts', args), error => error.check === 'serviceAccount' && error.result === 'permissionDenied');
  } else {
    assert.equal(JSON.stringify(await invoke('cluster_service_accounts', args)), JSON.stringify({ names: ['team-runner', 'task-reader'], truncated: false }));
    const preview = await invoke('cluster_service_account_preview', { ...args, saName: 'team-runner' });
    assert.equal(preview.status, 'allowed');
    assert.equal(preview.checks.map(check => `${check.resource}/${check.verb}/${check.status}`).join(), 'agents/create/allowed,agents/get/allowed,agents/list/allowed,tasks/create/allowed,tasks/get/allowed,tasks/list/allowed,providers/get/allowed,providers/list/allowed,tools/get/allowed,tools/list/allowed');
    await assert.rejects(invoke('cluster_service_account_preview', { ...args, saName: 'task-reader' }), /Only the selected account/);
  }
}
const reloadRecovery = source.slice(source.indexOf("if (name.startsWith('cluster-reload'))"), source.indexOf("if (name === 'model-copy') {"));
assert.ok(reloadRecovery.includes("document.querySelector('.cluster-card summary').click()"), 'connected recovery fixtures must reveal maintenance before reloading kubeconfig');

const clusterProbe = source.slice(source.indexOf("const probe = await evaluate(`({ heading: document.querySelector('h1')"), source.indexOf("throw Error('Connected cluster actions/wording:"));
assert.ok(clusterProbe.includes('summary[role="button"]'), 'connected actions include semantic summary disclosures, not only button elements');
assert.ok(!source.includes("await clickAction('Show details')") && !source.includes("await clickAction('Hide details')"), 'cluster disclosure uses its real summary, not the button-only helper');

// The connected scene must use the same real injection as discovery, then retain
// that selection for its success/details/list captures without another mount.
{
  let injection;
  const stop = new Error('injection captured');
  const capture = vm.runInNewContext(`(${captureActions})`, {
    pair: { agent: '', tests: '' }, replySavedAgent: '', GiB: 1024 ** 3, fixtureAccount: 'synthetic-user',
    call: async () => ({ frameTree: { frame: { id: 'fixture' } }, executionContextId: 1 }),
    evaluate: async script => { injection = script; throw stop; },
  });
  await assert.rejects(capture('light', 'wide', 'cluster-connected'), error => error === stop);
  const window = {};
  vm.runInNewContext(injection, { window, localStorage: { setItem() {}, removeItem() {} }, document: { getElementById: () => ({ replaceWith() {} }), createElement: () => ({}) } });
  const invoke = window.__TAURI_INTERNALS__.invoke;
  assert.equal((await invoke('cluster_places')).selectedKey, null);
  const [context] = await invoke('cluster_contexts');
  const [target] = await invoke('cluster_discover', { contextId: context.id });
  assert.equal((await invoke('cluster_use', { targetId: target.id, saName: null })).status, 'connected');
  const selected = await invoke('cluster_places');
  assert.equal(selected.places.find(place => place.key === selected.selectedKey).context, 'team-dev');
  assert.equal(selected.places.length, 3, 'selected, previously checked and unchecked rows stay distinct');
  assert.equal(selected.places.filter(place => place.checkedAt == null && place.observedAt === null).length, 1);
  const inventory = await invoke('overview_places', { key: selected.selectedKey });
  assert.equal(inventory.places[0].agents[0].name, 'runtime-reader');
  assert.equal(window.__bridgeClusterUses, 1, 'capture fixture must not replay Use');
}
for (const scene of ['cluster-success', 'cluster-details', 'cluster-selected-list']) {
  assert.ok(source.includes(`'${scene}'`), `Windows connected mount must capture ${scene}`);
}
const smoke = (await readFile(new URL('./native-smoke.swift', import.meta.url), 'utf8')).replaceAll('\r\n', '\n');
const frozenChecksumParts = smoke.match(/let checksumPattern = #"(.*?)"# \+ NSRegularExpression\.escapedPattern\(for: expected\) \+ #"(.*?)"#/)?.slice(1);
assert.ok(frozenChecksumParts, 'frozen test identity must match a whole identifier, not a longer token');
const matchesChecksum = text => new RegExp(`${frozenChecksumParts[0]}published-checksum${frozenChecksumParts[1]}`, 'i').test(text);
for (const text of ['published-checksum', 'PUBLISHED-CHECKSUM', '(published-checksum), Passed', 'Title published-checksum Passed']) assert.equal(matchesChecksum(text), true);
for (const text of ['published-checksum-extra', 'published-checksumX', 'other-published-checksum', 'published-checksum.extra', 'published-checksu']) assert.equal(matchesChecksum(text), false);
assert.equal(matchesChecksum(['published-', 'checksum'].reduce((text, line) => text + (text.endsWith('-') ? '' : ' ') + line)), true);
const cropPolicy = smoke.match(/let cropEligible = ([^\n]+)/)?.[1];
assert.ok(cropPolicy?.includes('expected == "1 of 1 tests passed"'), 'the exact passed count needs the same frozen-pixel OCR path as the small identifier');
const matchesPassedCount = text => new RegExp(`${frozenChecksumParts[0]}1 of 1 tests passed${frozenChecksumParts[1]}`, 'i').test(text);
for (const text of ['1 of 1 tests passed', '(1 of 1 tests passed).']) assert.equal(matchesPassedCount(text), true);
for (const text of ['l of 1 tests passed', '11 of 1 tests passed', '1 of 11 tests passed', '1 of 1 tests failed']) assert.equal(matchesPassedCount(text), false);
const countSpacing = smoke.match(/replacingOccurrences\(of: #"(.*?)"#, with: " of "/)?.[1];
assert.ok(countSpacing, 'Send count OCR must tolerate the observed missing space without accepting a different verdict');
const normalizedCount = text => text.replace(new RegExp(countSpacing, 'g'), ' of ').split(/\s+/).join(' ');
for (const text of ['1of 1 tests passed', '1 of1 tests passed', '1 of  1 tests passed']) assert.equal(normalizedCount(text), '1 of 1 tests passed');
assert.notEqual(normalizedCount('1of 1 tests failed'), '1 of 1 tests passed');
assert.notEqual(normalizedCount('2of 3 tests passed'), '1 of 1 tests passed');
const matrix = smoke.slice(smoke.indexOf('    // Connected cluster capture matrix.'), smoke.indexOf('    // Done leaves the confirmed destination'));
assert.ok(matrix.includes('CGFloat(540), CGFloat(640)'), 'Mac connected scenes need true minimum content size');
assert.ok(matrix.includes('CGFloat(1280), CGFloat(800)'), 'Mac connected scenes need matching wide content size');
for (const scene of ['cluster-success-', 'cluster-details-', 'cluster-selected-list-']) assert.ok(matrix.includes(scene), `Mac matrix must capture ${scene}`);
assert.ok(matrix.includes('"Show details"') && matrix.includes('"Hide details"'), 'capture both closed and open disclosure, then restore it');
assert.ok(smoke.includes('if name.hasPrefix("cluster-success")'), 'scrolled matrix frames need their own visible-content gate, not the off-screen page heading');
assert.ok(smoke.includes('nativeElements(element).dropFirst()'), 'named page h1 must also read its inline code accessibility child, not depend on a cluster-result h2');
assert.ok(smoke.includes('clickButton("Show details")\n    clickButton("Change context")'), 'context maintenance must explicitly open details');
assert.ok(smoke.includes('clickButton("Show details")\n    clickButton("Remove")'), 'restart removal must explicitly open details');
// Source contracts supplement, but never substitute for, the hosted native walk.
const model = (await readFile(new URL('./native-model-macos.swift', import.meta.url), 'utf8')).replaceAll('\r\n', '\n');
const captureManifest = JSON.parse(await readFile(new URL('./native-model-captures.json', import.meta.url), 'utf8'));
assert.ok(model.includes('let upgradeText = !found && name.hasPrefix("local-upgrade-") && nativeChatTextIsVisible(value, aboveFooter: true)'), 'upgrade OCR fallback must prove the unchanged expected substring in a fully visible native node');
assert.ok(model.includes('require(found || nativeText || upgradeText || chatText || workspaceControl || overviewControl || overviewHeading'), 'native upgrade evidence must feed the original hard capture assertion');
assert.ok(model.includes('theme == "dark" ? [9, 9, 10] : [255, 255, 255]') && model.includes('Native fixture does not use Orka paper'), 'native model captures must verify exact Orka paper in both themes');
assert.ok(model.includes('let pagePoint = CGPoint(x: (12 * CGFloat(image.width) / windowFrame.width).rounded(.down)') && model.includes('y: ((titlebarHeight + 2) * CGFloat(image.height) / windowFrame.height).rounded(.down))'), 'native theme sampling must use an integral pixel on the known page surface below the title bar');
assert.ok(model.includes('guard let capturedWindow = window() else') && model.includes('String(capturedWindow.0)') && model.includes('let windowFrame = capturedWindow.1'), 'native theme sampling must use the same window identity and geometry as its screenshot');
assert.ok(model.includes('var largest: (CGWindowID, CGRect)?') && model.includes('(candidate[kCGWindowOwnerPID as String] as? Int32) == child.processIdentifier') && model.includes('(candidate[kCGWindowLayer as String] as? Int) == 0') && model.includes('let area = rect.width * rect.height') && model.includes('if area > largestArea') && model.includes('largest = (id, rect)'), 'native capture must select the largest owned layer-zero window, not a transient auxiliary window');
assert.ok(model.includes('let pickerOpen = enabledButton("Close picker") != nil') && model.includes('let expected = zip(page, backdrop).map') && model.includes('let opacity: CGFloat = theme == "dark" ? 0.55 : 0.18') && model.includes('Native fixture picker backdrop does not use the Orka paper composite'), 'native model captures must strictly account for open picker backdrops over the known Orka page surface');
assert.ok(model.includes('CGImageSourceCreateWithURL') && model.includes('CGColorSpace(name: CGColorSpace.sRGB)'), 'native model theme sampling must measure rendered sRGB bytes, not display-calibrated AppKit colours');
assert.ok(model.includes('let overviewHeading = !found && value == "web-reader"') && model.includes('(name.hasPrefix("local-overview-") || name.hasPrefix("local-controls-overview-"))') && model.includes('["web-xeader", "WeD-reader"].contains(where: visible.contains) && nativeOverviewHeadingIsVisible(value)'), 'Overview captures must require an observed Geist OCR miss and exact web-reader native heading together');
assert.ok(model.includes('let accountValue = !found && value == "team-runner" && name.hasPrefix("cluster-account-allowed-")') && model.includes('["team- runner", "Team- Tunne t"].contains(where: visible.contains) && nativeControlValueIsVisible("ServiceAccount", value)'), 'Account captures must pair observed Geist OCR splits with the exact selected native control value');
assert.ok(model.includes('let accountExpectation = value == "team-runner" && name.hasPrefix("cluster-account-allowed-")') && model.includes('let chatText = !found && !accountExpectation'), 'the selected account must use its control-bound native proof, never the generic cluster text fallback');
assert.ok(model.includes('let suffix = value.dropFirst(label.count).trimmingCharacters') && model.includes('return suffix == expected'), 'native selected-control evidence may unwrap an accessibility label only when its remaining value is exact');
assert.ok(model.includes('nativeBounds(element).map { window()!.1.contains($0) && $0.maxY <= window()!.1.maxY - 32 } == true'), 'native selected-control evidence must exclude hidden and clipped duplicate controls before requiring uniqueness');
const nativeControlValue = model.slice(model.indexOf('func nativeControlValueIsVisible('), model.indexOf('func nativeOverviewHeadingIsVisible('));
assert.ok(nativeControlValue.includes('controls.count == 1') && nativeControlValue.includes('return value == expected || labelledValue(value)') && nativeControlValue.includes('|| nativeElements().contains { element in') && nativeControlValue.includes('["AXValue", "AXTitle", "AXDescription", "AXHelp"].contains') && nativeControlValue.includes('attribute(element, $0) as? String == expected') && nativeControlValue.includes('$0.width > 1 && $0.height > 1 && window()!.1.contains($0)') && nativeControlValue.includes('bounds.insetBy(dx: -2, dy: -2).contains($0)'), 'a described selected value must be an exact native string with real in-window bounds contained by the unique control');
const nativeOverviewHeading = model.slice(model.indexOf('func nativeOverviewHeadingIsVisible('), model.indexOf('func nativeChatTextIsVisible('));
assert.ok(nativeOverviewHeading.includes('enabledButton("Overview")') && nativeOverviewHeading.includes('bounds.minY >= headerBottom + 8') && nativeOverviewHeading.includes('bounds.maxY <= window()!.1.maxY - 32'), 'Overview heading fallback must stay inside the active page viewport and clear of fixed chrome');
assert.ok(smoke.includes('let backMiss = ["Pack to clusters", "back to clusters"].contains(where: text.contains)') && smoke.includes('backMiss && visibleButtons("Back to clusters").count == 1'), 'Manage captures must pair observed Geist Back OCR misses with the exact visible native button');
const nativeUpgradeVisibility = model.slice(model.indexOf('func nativeChatTextIsVisible('), model.indexOf('func waitForButton('));
assert.ok(nativeUpgradeVisibility.includes('text.contains(expected)') && nativeUpgradeVisibility.includes('"AXStaticText"'), 'upgrade fallback must use actual expected native text, not OCR spelling aliases');
assert.ok(nativeUpgradeVisibility.includes('bounds.width > 1 && bounds.height > 1 && window()!.1.contains(bounds)') && nativeUpgradeVisibility.includes('bounds.maxY <= window()!.1.maxY - 32'), 'hidden, clipped, off-window and footer text cannot satisfy upgrade fallback');
assert.equal(captureManifest.model.length, 208);
assert.equal(captureManifest['destinations-workspace'].length, 121);
assert.equal(new Set([...captureManifest.model, ...captureManifest['destinations-workspace']]).size, 329);
const overviewCaptures = [
  ...['disconnected', 'connected'].flatMap(ai => ['wide', 'minimum'].flatMap(size => ['collapsed', 'expanded'].map(state => `local-overview-ai-${ai}-${state}-${size}`))),
  ...['wide', 'compact', 'minimum'].flatMap(size => ['collapsed', 'expanded-earlier-pass', 'expanded-earlier-failed', 'expanded-draft'].map(state => `local-controls-overview-${state}-${size}`)),
];
assert.deepEqual(captureManifest.model.filter(name => name.includes('overview')).sort(), overviewCaptures.sort(), 'scoped manifest must retain every list/evidence capture with no obsolete grouping names');
const overviewDriver = source.slice(source.indexOf('  // Overview renderer fixtures:'), source.indexOf('  const notes = []'));
const expandOverview = overviewDriver.match(/await evaluate\(`(\[\.\.\.document\.querySelectorAll\('\.overview-agent-head[^`]+)`\);/)?.[1];
assert.ok(expandOverview, 'Overview must activate its real disclosure controls');
let overviewClicks = 0;
vm.runInNewContext(expandOverview, { document: { querySelectorAll(selector) {
  assert.equal(selector, '.overview-agent-head button[aria-label^="Show places for "][aria-expanded="false"]');
  return [{ click() { overviewClicks++; } }, { click() { overviewClicks++; } }];
} } });
assert.equal(overviewClicks, 2, 'the injected expression must parse and activate each collapsed row');
const overviewSeed = overviewDriver.match(/await evaluate\(`([\s\S]*?)`\);/)?.[1];
assert.ok(overviewSeed);
const overviewWindow = {};
vm.runInNewContext(overviewSeed.replace('${JSON.stringify(variant)}', JSON.stringify('starting')).replace('${JSON.stringify(pair)}', '{}'), {
  window: overviewWindow, localStorage: { setItem() {} }, document: { getElementById: () => ({ replaceWith() {} }), createElement: () => ({}) },
});
const longPlaces = await overviewWindow.__TAURI_INTERNALS__.invoke('overview_places');
assert.equal(longPlaces.places.length, 1, 'starting must exercise a segmented remote-place control');
assert.ok(longPlaces.places[0].context.length > 200, 'the native viewport case must include an unbroken long context label');
assert.ok(overviewDriver.includes(".overview-agent-head button[aria-label^=\"Show places for \"]"), 'Overview fixtures must drive the real agent disclosure contract');
assert.ok(overviewDriver.includes("document.querySelectorAll('.overview-place')"), 'Overview fixtures must prove place evidence is mounted only after expansion');
assert.ok(overviewDriver.includes("document.querySelector('[data-card-key=\"fixture-remote/remote-uid-1\"]')"), 'expanded places must retain exact Card keys');
assert.ok(overviewDriver.includes("getAttribute('aria-label')"), 'place accessibility labels must be accepted by the capture gate');
assert.ok(overviewDriver.includes("card?.querySelectorAll('button, [role=\"button\"]')"), 'Overview action proof must include summary-based Details controls');
assert.ok(overviewDriver.includes('oldCards:') && overviewDriver.includes('oldGroups:') && !overviewDriver.includes("clickAction('By agent')") && !overviewDriver.includes("clickAction('By cluster')"), 'obsolete grouping/card controls must be rejected by the Overview fixture');
assert.ok(overviewDriver.includes("shoot('-collapsed')") && overviewDriver.includes("shoot('-expanded')"), 'Overview must capture collapsed and expanded native states');
assert.ok(model.includes('local-overview-ai-disconnected-collapsed-') && model.includes('local-overview-ai-connected-expanded-'), 'native model fixtures must cover disconnected and connected list states');
const overviewResult = model.slice(model.indexOf('func nativeOverviewResultIsVisible('), model.indexOf('func nativeChatTextIsVisible('));
assert.ok(overviewResult.includes('enabledButton("Show places for \\(agent)")') && overviewResult.includes('enabledButton("Show places for \\(nextAgent)")') && overviewResult.includes('attribute(element, "AXRole") as? String == "AXStaticText"') && overviewResult.includes('attribute(element, "AXValue") as? String == expected') && overviewResult.includes('bounds.width > 1 && bounds.height > 1 && window()!.1.contains(bounds)') && overviewResult.includes('bounds.minY >= start.maxY && bounds.maxY <= end.minY'), 'native Overview result evidence must be exact, visibly bounded and lie between its agent row and the following row');
assert.ok(model.includes('theme == "dark" && size == "wide" && scene == "earlier-failed"') && model.includes('pixels.contains("lot & passed, derore your edle")') && model.includes('pixels.contains("Failed") && pixels.contains("Run tests")') && model.includes('nativeOverviewResultIsVisible("pr-summary", before: "release-notes", "1 of 2 passed, before your edit")') && model.includes('expected.allSatisfy { pixels.contains($0) } || failedCountMiss'), 'only the observed dark wide Overview verdict OCR miss may use exact card-scoped native evidence');
assert.ok(!model.includes('by-cluster') && !model.includes('by-agent'), 'native model fixtures must not retain grouping controls');
assert.ok(model.includes('Cannot bring Overview disclosure into view') && model.includes('let top = enabledButton("Overview").flatMap(nativeBounds)?.maxY'), 'native Overview disclosures must be brought clear of fixed navigation before pointer activation');
assert.ok(model.includes('capture("local-overview-ai-disconnected-collapsed-\\(size)", ["Your agents"])') && model.includes('require(nativeResultTextIsVisible("Connect the AI in Clusters to chat or run tests on this computer.")'), 'disconnected Overview must verify the exact native reason without fragile OCR of AI/Al');
assert.ok(smoke.includes('Show places for web-reader') && smoke.includes('overview-fixture-multiple-places-expanded'), 'native smoke must expand the agent list before proving per-place evidence');
assert.ok(!smoke.includes('heading: "fixture-secondary"'), 'place names are marks or evidence labels, never list headings');
assert.ok(smoke.includes('overviewShot("overview-fixture-offline-expanded", ["fixture-secondary:",'), 'offline recovery must anchor its source label, not a distant place mark');
const overviewShotGate = smoke.slice(smoke.indexOf('    func overviewShot('), smoke.indexOf('    func recordShot('));
assert.ok(overviewShotGate.includes('afterFrame:') && !overviewShotGate.includes('anchor.minY'), 'capture the already-visible native Overview state; do not scroll it away for cosmetic framing');
assert.ok(smoke.includes('name.hasPrefix("overview-probe-")'), 'renamed scrolled Overview probes must retain the existing heading exemption');
assert.ok(smoke.includes('CGFloat(image.width - chromeInset)') && smoke.includes('name.hasPrefix("overview-") ? 20 : 8'), 'Overview theme sampling must avoid its transient edge scrollbar without relaxing the theme tolerance');
assert.ok(smoke.includes('theme == "dark" ? [9, 9, 10] : [255, 255, 255]') && smoke.includes('Orka paper'), 'native smoke must verify the Orka paper palette in both themes');
assert.ok(source.includes("theme === 'dark' ? '#09090a' : '#ffffff'") && source.includes("theme === 'dark' ? '#f4f4f5' : '#0b0b0c'"), 'Windows comparisons must use Orka paper and ink');
assert.ok(!smoke.includes('clickText("Refresh"') && !smoke.includes('clickButton("Refresh")') && !smoke.includes('clickText("By agent"') && !smoke.includes('clickText("By cluster"'), 'native smoke must remove top Refresh and grouping-only Overview paths');
for (const name of ['chat-long-early-wide', 'chat-long-early-minimum', 'chat-long-end-wide', 'chat-long-end-minimum']) {
  assert.ok(captureManifest.model.includes(name), `long Chat capture must belong to model scope: ${name}`);
  assert.ok(!captureManifest['destinations-workspace'].includes(name), `long Chat capture must not leak into destination scope: ${name}`);
}
assert.ok(model.includes('nativeHeadingExists("Conversation 1") && nativeHeadingExists("Conversation 2")'), 'numbered Chat versions must be verified as native headings, not body static text');
assert.ok(model.includes('anchorChatHeading("Conversation 2")'), 'Chat capture must align its native heading clear of fixed navigation');
assert.ok(model.includes('let delta = header.maxY + 64 - bounds.minY'), 'Chat heading navigation must use actual header/heading geometry');
assert.ok(smoke.indexOf('try capture("send-history-answer", "Designer")') > smoke.indexOf('let savedWide = try capturePreflight'), 'offline answer proof must follow its wide resize and provenance anchor');
assert.ok(model.includes('attribute(element, "AXRole") as? String == "AXHeading"'), 'Chat heading discovery must use its actual native role');
assert.ok(smoke.includes('("minimum", CGFloat(540), CGFloat(640))'));
for (const name of ['send-failed', 'send-unsupported', 'send-earlier-target']) assert.ok(smoke.includes(`captureSendPresentation("${name}"`));
assert.ok(smoke.includes('Native') && smoke.includes('Result window size was not applied'));
const unsupportedWalk = smoke.slice(smoke.indexOf('    // Unsupported is deliberately the last Send:'), smoke.indexOf('    // Reopen without a Send.'));
assert.ok(unsupportedWalk.includes('"Check cluster status"'));
assert.ok(!unsupportedWalk.includes('clickSendControl("Retry cancellation"'), 'unsupported must be last, never followed by retry');
assert.ok(smoke.indexOf('send-cancel-completion-race') < smoke.indexOf('// Unsupported is deliberately the last Send:'));
assert.ok(smoke.includes('Resources and file hashes'), 'native discovery must distinguish resources from nested Details');
assert.ok(smoke.includes('let buttonAnchor = nativeButtons(anchor ?? expected, root: root)'), 'action captions need scoped AXButton bounds after Details closes');
assert.ok(smoke.includes('nativeTextBounds(anchor ?? expected, visibleOnly: false, exact: anchor != nil, root: root)'), 'duplicate local test names cannot steer Send anchors');
assert.ok(smoke.includes('nativeTextBounds(failureReason, exact: true, root: root)'), 'the visible failure reason must belong to Send');
assert.ok(smoke.includes('nativeButtons("has-a-date", root: root).compactMap'), 'case identity/visibility must use its native summary control, not a title static-text proxy');
assert.ok(smoke.includes('let text = scopedSeen.compactMap'), 'frozen OCR acceptance must use region-bound evidence, not the matching local test');
assert.ok(smoke.includes('attribute(element, "AXSubrole") as? String == "AXLandmarkRegion"'), 'named regions must not collide with their disclosure groups');
assert.ok(smoke.includes('return viewport.contains(textBounds)'), 'frozen OCR evidence must be contained in the visible native region');
assert.ok(smoke.includes('text.hasSuffix("-")'), 'hyphenated fixture IDs must survive native OCR line wraps');
assert.ok(smoke.includes('region: "Past Sends", requiring:'), 'saved history anchors keep their separate region');
assert.ok(smoke.includes('scrolling: -350, region: nil'), 'the Designer Send entry is outside the result region');
assert.ok(smoke.includes('if let anchor = textAnchor ?? buttonAnchor'), 'native action anchors retain frozen-frame verification');
assert.ok(smoke.includes('func uniqueOCRTarget('), 'scoped OCR click selection must reject distinct targets');
assert.ok(smoke.includes('let scopedSeen = seen.filter'), 'OCR click candidates must belong to the selected native region');
assert.ok(smoke.includes('clickText(control, in: [target])'), 'clicking must not search the full frame again');
assert.ok(smoke.includes('matches.allSatisfy({ $0 == first })'), 'partly overlapping OCR boxes must not be coalesced as one target');
for (const name of ['OCR excludes outside-region duplicates', 'OCR rejects distinct in-region targets', 'OCR rejects partial-overlap targets', 'OCR preserves one in-region target']) {
  assert.ok(smoke.includes(name), `native geometry regression missing: ${name}`);
}
const checksumPattern = smoke.match(/label == "published-checksum" && text\.range\(of: #"([^\"]+)"#/)[1];
const checksumLabel = new RegExp(checksumPattern, 'i');
assert.ok(checksumLabel.test('› published-checksum'));
assert.ok(checksumLabel.test('Copies the checksum from the pagepublished-checksum'), 'OCR can join captured title and stable test ID');
assert.ok(!checksumLabel.test('Copies a different checksum'));
assert.ok(model.includes('localSize(540, 640)') && model.includes('chat-new-version-minimum'));
assert.ok(!model.includes('seek("Ask your agent a question"'), 'empty Chat has no invitation to seek');
assert.ok(model.includes('Enter sends · Shift+Enter for a new line'), 'Mac capture must check the precise composer hint');
assert.ok(model.includes('requireChatComposerVisible()'), 'Mac captures must verify composer geometry');
assert.ok(model.includes('chat-long-early-') && model.includes('chat-long-end-'), 'Mac long Chat must capture earlier-reader and end states');
assert.ok(model.includes('name.hasPrefix("local-shell-chat-")'), 'mixed local/Chat captures must check the visible native subtitle, not only OCR');
assert.ok(smoke.includes('"Fix in Designer", "Run cluster tests again"'), 'failed Send must preserve Fix primary and precise rerun secondary');
assert.ok(source.includes('chat-long-early') && source.includes('chat-long-end'), 'long Chat needs distinct earlier-reader and end captures');
assert.ok(source.includes("resizeTo({ width: 1280, height: 800 }, 'Wide')"));
assert.ok(source.includes("resizeTo({ width: 540, height: 640 }, 'Compact')"));
assert.ok(!source.includes('Emulation.setDeviceMetricsOverride'), 'a fake viewport cannot qualify native size evidence');
// Upgrade presentation extends existing scope manifests, not the native safety proof.
const upgradeManifest = JSON.parse(await readFile(new URL('./native-model-captures.json', import.meta.url), 'utf8'));
const upgradeNames = upgradeManifest['destinations-workspace'].filter(name => name.startsWith('local-upgrade-'));
const integrationStates = ['refresh-cached', 'refresh-pending', 'refresh-busy', 'refresh-io', 'cancel-requested', 'cancel-completed', 'cancel-not-cancellable', 'forward-cancelled', 'busy-locked', 'incomplete-forced'];
const integrationNames = integrationStates.flatMap(state => ['wide', 'compact'].map(size => `local-upgrade-manage-${state}-${size}`));
assert.equal(upgradeNames.length, 70, '50 preserved upgrade frames plus20 genuinely new owner-contract frames');
assert.equal(new Set(upgradeNames).size, 70);
assert.equal(upgradeNames.filter(name => !integrationNames.includes(name)).length, 50, 'both sizes of original setup/Manage states and confirmations remain');
for (const name of integrationNames) assert.ok(upgradeNames.includes(name), `missing integration frame: ${name}`);
assert.ok(!upgradeManifest.model.some(name => name.startsWith('local-upgrade-')), 'model journey must not duplicate upgrade captures');
const upgradeCapture = source.slice(source.indexOf('  async function captureUpgrade('), source.indexOf('  const notes = [];'));
const upgradePrelude = upgradeCapture.slice(upgradeCapture.indexOf("    const transaction"), upgradeCapture.indexOf("    for (const [localVm"));
const upgradeVariants = Function(upgradePrelude + '\nreturn variants;')();
assert.deepEqual(upgradeVariants.slice(0, 20).map(row => row[1]), [
  'upgrade-eligible-absent', 'upgrade-eligible-credential', 'upgrade-incomplete',
  'upgrade-source-selected', 'upgrade-source-retained', 'upgrade-restoring', 'upgrade-restoring-confirm',
  'upgrade-fresh-absent', 'upgrade-fresh-eligible', 'upgrade-fresh-attention', 'upgrade-fresh-removal-confirm',
  'upgrade-fresh-transferred', 'upgrade-fresh-removed', 'upgrade-consumed-restore-confirm',
  'upgrade-restored-freshc-confirm', 'upgrade-conflict', 'upgrade-error-network', 'upgrade-error-nospace',
  'upgrade-error-stop', 'upgrade-busy-cancel',
], 'all 20 Windows variants retain their exact existing names');
assert.deepEqual(upgradeVariants.slice(20).map(row => row[1]), ['upgrade-busy-locked', 'upgrade-error-forced', 'upgrade-refresh-pending', 'upgrade-refresh-busy', 'upgrade-refresh-io', 'upgrade-cancel-requested', 'upgrade-cancel-completed', 'upgrade-cancel-not-cancellable', 'upgrade-forward-cancelled']);
assert.equal(upgradeVariants.length, 29);
// Every pre-confirmation frame remains allocated, but a name ending -confirm
// can only be written with that exact filename after its fieldset has opened.
const observationName = upgradeCapture.match(/const observationName = ([^;]+);/)[1];
const confirmationName = upgradeCapture.match(/const confirmationName = ([^;]+);/)[1];
const frameNames = [];
for (const [, name, , , mode] of upgradeVariants) {
  frameNames.push(Function('name', 'mode', `return ${observationName}`)(name, mode));
  if (mode) {
    const confirmed = Function('name', `return ${confirmationName}`)(name);
    assert.ok(confirmed.endsWith('-confirm'));
    assert.ok(!confirmed.endsWith('-confirm-confirm'));
    if (name.endsWith('-confirm')) assert.equal(confirmed, name);
    frameNames.push(confirmed);
  }
}
assert.equal(frameNames.length, 36, '27 baseline frame slots plus nine new scenarios; cached freshness adds one separately');
assert.equal(new Set(frameNames).size, frameNames.length, 'Windows upgrade screenshot filenames must not collide');
assert.ok(upgradeCapture.indexOf('const confirmationName') > upgradeCapture.indexOf('confirmCount !== 1'), 'exact confirmation filename must follow the actual open fieldset assertion');
assert.ok(model.includes('Integration keyboard focus was lost'), 'new owner-contract frames retain hard keyboard focus checks');
assert.ok(model.includes('Cached observation erased manual refresh error'), 'Mac must check sticky owner-query errors after memory polling');
assert.ok(!upgradeCapture.includes("clickAction('Stop"), 'upgrade fixtures must not navigate through removed Stop');
assert.ok(upgradeCapture.includes("clickAction('Manage')"), 'returning upgrade scenes must enter Manage directly');
assert.ok(upgradeCapture.includes("localStorage.setItem('bridge.firstTest.v1', 'attempted')"), 'returning upgrade scenes must own retired first-test intent');
assert.ok(upgradeCapture.includes("localStorage.removeItem('bridge.firstTest.v1')"), 'initial upgrade scenes must retire previous returning intent');
assert.ok(upgradeVariants.slice(0, 3).every(row => row[4]), 'every initial upgrade scene requires a confirmation capture');
assert.ok(model.includes('if manage && index == 0 { waitForButton("Manage"); click("Manage", []) }'), 'fresh returning renderer must explicitly open Manage before observing its card');
assert.ok(model.includes('event?.flags = backwards && down ? .maskShift : []'), 'each Tab gesture must set its modifiers and release Shift');
const readyRow = source.match(/await until\(\(\) => evaluate\(`(\['Connect the AI', 'Manage'\][^`]+)`\), 'ready local row'\)/)?.[1];
assert.ok(readyRow, 'ready fixtures must wait for an observed snapshot, not merely an initial Set up button');
for (const [label, expected] of [['Set up', false], ['Connect the AI', true], ['Manage', true]]) {
  assert.equal(vm.runInNewContext(readyRow, { document: { querySelector: () => ({ textContent: label }) } }), expected);
}
assert.ok(model.includes('require(currentFocus, "Confirmation keyboard focus was lost")'), 'Mac must retain its hard keyboard focus assertion');
assert.ok(model.includes('Thread.sleep(forTimeInterval: 0.2)\n                // AX nodes'), 'Mac keyboard deadline must not be extended to pass');
const cachedFilename = upgradeCapture.match(/await writeFile\(join\(output, (`[^`]*upgrade-refresh-cached\.png`)/)?.[1];
assert.ok(cachedFilename, 'cached observation must still write its separate frame');
const cachedName = Function('theme', 'size', `return ${cachedFilename};`)('light', 'wide');
assert.equal(cachedName, 'light-wide-upgrade-refresh-cached.png');
const upgradeSlots = [...frameNames, cachedName.replace('light-wide-', '').replace('.png', '')];
assert.equal(upgradeSlots.length, 37, '27 baseline plus ten integration frames per theme/size');
assert.equal(new Set(upgradeSlots).size, 37);
assert.equal(frameNames.filter(name => !upgradeVariants.slice(20).some(row => row[1] === name)).length, 27);
const scopeSlots = new Map(['core', 'chat-agent-clusters', 'workspace-upgrade', 'all'].map(scope => [scope, []]));
for (const theme of ['light', 'dark']) for (const size of ['wide', 'compact']) {
  for (const scope of scopeSlots.keys()) {
    const chosen = selection(scope);
    const actionNames = actions(chosen).map(name => `${theme}-${size}-${name}`);
    assert.equal(actionNames.length, { core: 47, 'chat-agent-clusters': 25, 'workspace-upgrade': 0, all: 72 }[scope]);
    const partition = ['core', 'chat-agent-clusters', 'workspace-upgrade'].flatMap(part => actions(selection(part))).map(name => `${theme}-${size}-${name}`);
    assert.deepEqual(partition, actions(selection('all')).map(name => `${theme}-${size}-${name}`));
    assert.equal(new Set(partition).size, 72);
    for (const [, name] of upgradeVariants) {
      const calls = [], stop = new Error('upgrade world boundary');
      const capture = Function('runUpgrade', 'selectedFixture', 'frameTree', 'call',
        `${upgradeCapture.replace('of variants)', 'of variants.filter(v => v[1] === selectedFixture))')}; return captureUpgrade;`)(
        chosen.runUpgrade, name, { frame: { id: 'fixture-frame' } }, async (method, args) => { calls.push([method, args]); throw stop; });
      if (scope === 'core' || scope === 'chat-agent-clusters') {
        await capture(theme, size);
        assert.equal(calls.length, 0, `${scope} must not duplicate any upgrade world`);
      } else {
        await assert.rejects(capture(theme, size), error => error === stop);
        assert.equal(calls.length, 1, `${scope} must admit every upgrade variant`);
        assert.equal(calls[0][0], 'Page.createIsolatedWorld');
        assert.equal(calls[0][1].worldName, `upgrade-${theme}-${size}-${name}`);
      }
    }
    if (chosen.runUpgrade) scopeSlots.get(scope).push(...upgradeSlots.map(name => `${theme}-${size}-${name}.png`));
  }
}
assert.equal(scopeSlots.get('workspace-upgrade').length, 148);
assert.deepEqual(scopeSlots.get('workspace-upgrade'), scopeSlots.get('all'));
const splitSlots = ['core', 'chat-agent-clusters', 'workspace-upgrade'].flatMap(scope => scopeSlots.get(scope));
assert.deepEqual(splitSlots, scopeSlots.get('all'), 'split scopes preserve every upgrade frame exactly once');
assert.equal(new Set(splitSlots).size, 148, 'no theme/size/frame overwrites across split scopes');
// Execute the driver's actual resize/theme loops: a separately invented size list
// cannot detect compact captures accidentally overwriting wide artifacts.
async function upgradeRoutes(driver) {
  const wide = driver.slice(driver.indexOf('  const notes = [];'), driver.indexOf('  // Request CSS viewport dimensions'));
  const compact = driver.slice(driver.indexOf("  await resizeTo({ width: 540, height: 640 }, 'Compact');"), driver.indexOf('  // Preserve the explicitly named Overview minimum'));
  assert.ok(wide.includes('captureUpgrade') && compact.includes('captureUpgrade'));
  const routes = []; let viewport;
  await vm.runInNewContext(`(async () => { ${wide}\n${compact} })()`, {
    setupFixtures: [], actionFixtures: [], overviewVariants: [],
    resizeTo: async target => { viewport = target; }, call: async () => {},
    captureUpgrade: async (theme, size) => routes.push([theme, size, viewport.width, viewport.height]),
  });
  assert.deepEqual(routes, [
    ['light', 'wide', 1280, 800], ['dark', 'wide', 1280, 800],
    ['light', 'compact', 540, 640], ['dark', 'compact', 540, 640],
  ]);
}
await upgradeRoutes(source);
await assert.rejects(upgradeRoutes(source.replace("await captureUpgrade(theme, 'compact')", "await captureUpgrade(theme, 'wide')")), /deep-equal/);
await assert.rejects(upgradeRoutes(source.replace("await captureUpgrade(theme, 'wide')", "await captureUpgrade(theme, 'compact')")), /deep-equal/);
// The capture is passive even if React catches a rejected mutation reply.
const upgradeAudit = upgradeCapture.slice(upgradeCapture.indexOf('      for (const { command, args }'), upgradeCapture.indexOf('      await checkLayout('));
assert.ok(upgradeAudit.includes('workspace_upgrade_'), 'upgrade call audit is missing');
const auditUpgrade = Function('calls', 'name', 'transaction', upgradeAudit);
for (const args of [null, {}]) assert.doesNotThrow(() => auditUpgrade([{ command: 'workspace_upgrade_status', args }], 'fixture', 'fixture-transaction'));
for (const args of [{ path: 'unexpected' }, { transaction: 'fixture-transaction' }, { refresh: true }, { refresh: false }, [], 'unexpected']) {
  assert.throws(() => auditUpgrade([{ command: 'workspace_upgrade_status', args }], 'fixture', 'fixture-transaction'));
}
// Seven real command names: six mutations plus the explicit-refresh status boundary.
for (const action of ['status', 'replace', 'retry', 'restore', 'cancel', 'transfer', 'remove']) {
  for (const args of [{ transaction: 'fixture-transaction' }, { refresh: true }, { outcome: 'completed', status: {} }]) {
    assert.throws(() => auditUpgrade([{ command: `workspace_upgrade_${action}`, args }], 'fixture', 'fixture-transaction'), 'even a well-shaped caught mutation must fail the capture');
  }
  if (action !== 'status') for (const args of [null, {}]) {
    assert.throws(() => auditUpgrade([{ command: `workspace_upgrade_${action}`, args }], 'fixture', 'fixture-transaction'));
  }
}
for (const name of ['upgrade-refresh-pending', 'upgrade-refresh-busy', 'upgrade-refresh-io']) {
  assert.throws(() => auditUpgrade([], name, 'fixture-transaction'), /exactly one owner refresh/);
  assert.throws(() => auditUpgrade(Array(2).fill({ command: 'workspace_upgrade_status', args: { refresh: true } }), name, 'fixture-transaction'), /exactly one owner refresh/);
  assert.doesNotThrow(() => auditUpgrade([
    { command: 'workspace_upgrade_status', args: null },
    { command: 'workspace_upgrade_status', args: { refresh: true } },
    { command: 'workspace_upgrade_status', args: {} },
  ], name, 'fixture-transaction'), 'cached reads remain allowed around the one explicit owner check');
  for (const args of [{ refresh: true, path: 'unexpected' }, { refresh: false }, { refresh: true, transaction: 'fixture-transaction' }]) {
    assert.throws(() => auditUpgrade([{ command: 'workspace_upgrade_status', args }], name, 'fixture-transaction'));
  }
  assert.throws(() => auditUpgrade([{ command: 'workspace_upgrade_restore', args: { transaction: 'fixture-transaction' } }], name, 'fixture-transaction'));
}
assert.throws(() => auditUpgrade([{ command: 'workspace_upgrade_status', args: { refresh: true } }], 'upgrade-refresh-arbitrary', 'fixture-transaction'));
// Old isolated-world React pollers may still retire their own first-test intent;
// their synthetic preferences must not change a later scene's identity.
const fixtureStorage = source.match(/const fixtureStorage = `([\s\S]*?)`;/)?.[1];
assert.ok(fixtureStorage, 'each renderer fixture world needs isolated presentation preferences');
const nativePreferences = new Map([['native-sentinel', 'kept']]);
const nativeStorage = { getItem: key => nativePreferences.get(key) ?? null, setItem: (key, value) => nativePreferences.set(key, value), removeItem: key => nativePreferences.delete(key) };
const worlds = [0, 1].map(() => ({ window: { localStorage: nativeStorage } }));
for (const world of worlds) vm.runInNewContext(fixtureStorage, world);
worlds[0].window.localStorage.setItem('bridge.firstTest.v1', 'attempted');
assert.equal(worlds[1].window.localStorage.getItem('bridge.firstTest.v1'), null);
worlds[1].window.localStorage.setItem('bridge.firstTest.v1', 'pending');
worlds[0].window.localStorage.setItem('bridge.firstTest.v1', 'attempted');
assert.equal(worlds[1].window.localStorage.getItem('bridge.firstTest.v1'), 'pending');
assert.deepEqual([...nativePreferences], [['native-sentinel', 'kept']], 'renderer fixtures must not touch the native profile storage');
// Execute the fixture's document-start handshake: old-page callbacks must not
// overwrite the next renderer's intended setup/Manage presentation.
const upgradeFixture = await readFile(new URL('../app/src-tauri/examples/model_walk.rs', import.meta.url), 'utf8');
assert.ok(upgradeFixture.includes('assert!(expected_call, "Unexpected upgrade capture call:'), 'Mac must fail even caught passive mutations and nonempty status args');
assert.ok(upgradeFixture.includes('args.len() == 1') && upgradeFixture.includes('Some(&json!(true))'), 'Mac manual refresh must use the exact finite refresh:true argument');
assert.ok(upgradeFixture.includes('if key == "sourceRetained"') && upgradeFixture.includes('40000000-0000-4000-8000-000000000005'), 'visually identical retained scene needs a distinct synthetic transaction witness');
assert.ok(model.includes('if state == "sourceRetained"') && model.includes('waitForNativeText(["4000…0005"])'), 'Mac must observe the requested scene in rendered details before naming its screenshot');
const nativeTextWait = model.slice(model.indexOf('func waitForNativeText(_ any:'), model.indexOf('func reachable('));
assert.ok(nativeTextWait.includes('for _ in 0..<40') && nativeTextWait.includes('Thread.sleep(forTimeInterval: 0.1)'), 'the native text deadline must remain unchanged');
for (const size of ['1.2 GB', '1.3 GB']) assert.ok(model.includes(`waitForNativeText(["${size}"])`), 'Mac allocation witnesses must match one rendered AX text value, not an interpolated paragraph');
const integrationStart = model.indexOf('for state in ["refresh-pending"');
const integrationEntry = model.slice(integrationStart, model.indexOf('    if state.hasPrefix("refresh-") {', integrationStart));
const buttonCondition = integrationEntry.match(/if (.+) \{ waitForButton\(expected\) \}/)?.[1];
assert.ok(buttonCondition, 'Mac integration action anchors must wait for AXButton, not AXStaticText');
for (const state of ['refresh-pending', 'refresh-busy', 'refresh-io', 'cancel-requested', 'cancel-completed', 'cancel-not-cancellable', 'forward-cancelled', 'busy-locked', 'incomplete-forced']) {
  const button = vm.runInNewContext(buttonCondition.replaceAll('.hasPrefix(', '.startsWith('), { state });
  assert.equal(button, !['busy-locked', 'incomplete-forced'].includes(state), `${state}: action and static-text witness roles must remain distinct`);
}
const upgradeBootstrap = upgradeFixture.match(/const UPGRADE_INITIALIZE: &str = r#"([\s\S]*?)"#;/)?.[1];
assert.ok(upgradeBootstrap, 'fixture intent must be restored at document start, not before old callbacks finish');
for (const intent of ['pending', 'attempted']) {
  const local = new Map([['bridge.firstTest.v1', intent === 'pending' ? 'attempted' : 'pending']]);
  const session = new Map([['bridge.fixture.upgradeIntent', intent]]);
  const storage = map => ({ getItem: key => map.get(key) ?? null, setItem: (key, value) => map.set(key, value), removeItem: key => map.delete(key) });
  const context = { localStorage: storage(local), sessionStorage: storage(session) };
  vm.runInNewContext(upgradeBootstrap, context);
  assert.equal(local.get('bridge.firstTest.v1'), intent);
  assert.equal(session.has('bridge.fixture.upgradeIntent'), false);
  local.set('bridge.firstTest.v1', 'attempted');
  vm.runInNewContext(upgradeBootstrap, { ...context });
  assert.equal(local.get('bridge.firstTest.v1'), 'attempted', 'bootstrap must not rearm intent after the handshake');
}
await import('./test-native-workspace-contracts.mjs');
console.log('PASS real scope parsing; 47+25 unique action cases; 29 upgrade variants/148 disjoint Windows frame slots; upgrade scope/50 baseline+20 integration-name matrix; connected cluster IPC/matrix; Chat/Send snapshot matrix and presentation gates; Swift source contracts; layout acceptance/rejection; IPC registration/injection; disjoint CI scopes with unchanged three-minute ceilings');
