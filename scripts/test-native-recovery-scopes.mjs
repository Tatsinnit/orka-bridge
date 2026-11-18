// Exercise the real driver's argument parsing and fixture rosters without launching Windows.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import vm from 'node:vm';

const source = (await readFile(new URL('./native-recovery-windows.mjs', import.meta.url), 'utf8')).replaceAll('\r\n', '\n');
const prefix = source.slice(0, source.indexOf('for (const root of')).replace(/^import .*;\n/gm, '');
function selection(scope) {
  const argv = ['node', 'driver', 'app.exe', 'captures', ...(scope === undefined ? [] : [scope])];
  return vm.runInNewContext(`${prefix}\n({ executable, output, scope: typeof captureScope === 'undefined' ? undefined : captureScope, runCore: typeof runCore === 'undefined' ? true : runCore, runUi: typeof runUi === 'undefined' ? true : runUi })`, {
    process: { platform: 'win32', env: { GITHUB_ACTIONS: 'true' }, argv }, resolve,
  });
}
assert.throws(() => selection('typo'), /capture scope/i, 'unknown scope must not silently run the full suite');
assert.equal(selection().scope, 'all');
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
assert.equal(all.length, 67);
assert.equal(core.length, 47);
assert.equal(ui.length, 20);
assert.ok(core.includes('designer-reply-saved'), 'saved reply captures belong to the core scope');
assert.ok(!ui.includes('designer-reply-saved'), 'UI scope must not repeat saved reply captures');
assert.equal(new Set([...core, ...ui]).size, all.length, 'scopes must partition, not repeat or omit cases');
assert.deepEqual([...core, ...ui], all);
assert.deepEqual(ui, ['chat-empty', 'chat-pending', 'chat-running', 'chat-answer', 'chat-session-null', 'chat-session-answer', 'chat-two-versions', 'send-failed', 'send-unsupported', 'send-earlier-target', 'new-agent-form', 'new-agent-created', 'cluster-picker-open', 'cluster-checking', 'cluster-failure', 'cluster-connected', 'cluster-account-picker-open', 'cluster-account-allowed', 'cluster-account-forbidden', 'designer-no-destination']);
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
const twoVersions = Function('name', 'documentId', chatStatus)('chat-two-versions', 'fixture-document');
assert.equal(twoVersions.versions.length, 2);
assert.equal(twoVersions.versions[0].turns[0].message, 'Keep this earlier conversation.');
assert.equal(twoVersions.versions[1].turns[0].message, 'What did the page say?');
for (const name of ['fixtures', 'setupFixtures']) {
  assert.equal(roster(name, selection('chat-agent-clusters')).length, 0, `UI scope must not repeat ${name}`);
  assert.ok(roster(name, selection('core')).length > 0);
}
const overviewAll = roster('overviewVariants', selection());
const overviewCore = roster('overviewVariants', selection('core'));
const overviewUi = roster('overviewVariants', selection('chat-agent-clusters'));
assert.deepEqual(overviewAll, ['clusters', 'local-only', 'starting']);
assert.deepEqual(overviewCore, [], 'Overview must leave the constrained Core scope');
assert.deepEqual(overviewUi, overviewAll, 'UI must own the complete Overview matrix');
assert.deepEqual([...overviewCore, ...overviewUi], overviewAll, 'split Overview coverage must match all');
assert.equal(new Set([...overviewCore, ...overviewUi]).size, overviewAll.length, 'Overview must not repeat between scopes');
assert.match(source, /if \(runCore\) for \(const name of \['designer-connected'/, 'UI scope must not repeat minimum Designer cases');
// Execute the capture gate with accepted and violating observations. This tests
// the gate, not native rendering; CI supplies the actual DOM and window measurements.
const sendGate = source.slice(source.indexOf('  function assertSendPresentation('), source.indexOf('  async function captureSendScene('));
assert.ok(sendGate.includes('Unsupported cancellation'), 'Send capture acceptance gate is missing');
const checkSend = Function(`${sendGate}; return assertSendPresentation;`)();
const sendProbe = { header: [{ label: 'Run cluster tests again', primary: true }, { label: 'See in Overview', primary: false }],
  buttons: ['Run cluster tests again', 'See in Overview'], title: 'Captured checksum from the page', id: 'page-checksum',
  text: 'fixture-captured', detailsOpen: false, extras: [], width: 540, scrollWidth: 540 };
checkSend('send-failed', sendProbe, false);
const unsupported = { ...sendProbe, header: [{ label: 'Check cluster status', primary: true }], buttons: ['Check cluster status'] };
checkSend('send-unsupported', unsupported, false);
checkSend('send-earlier-target', { ...sendProbe, header: [{ label: 'Send and run tests', primary: true }, sendProbe.header[1]], text: 'Earlier result on fixture-captured Destination fixture-selected' }, false);
checkSend('send-failed', { ...sendProbe, detailsOpen: true, extras: ['Check again', 'Send as a separate agent'] }, true);
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
  scrollbar: { width: 'thin', color: 'rgb(90, 90, 90) rgb(240, 240, 240)' }, fallback: {} };
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
for (const [scope, directory] of [['core', 'native-recovery'], ['chat-agent-clusters', 'native-chat-agent-clusters']]) {
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
for (const name of ['cluster-account-picker-open', 'cluster-account-allowed', 'cluster-account-forbidden', 'designer-no-destination', 'send-failed', 'send-unsupported', 'send-earlier-target']) {
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
  const [target] = await invoke('cluster_discover');
  if (name.startsWith('send-')) {
    const { documentId, view, busy } = await invoke('send_status');
    const preflight = await invoke('send_preflight');
    const opened = await invoke('agent_open');
    assert.equal(busy, null);
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
const model = await readFile(new URL('./native-model-macos.swift', import.meta.url), 'utf8');
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
assert.ok(source.includes("resizeTo({ width: 1280, height: 800 }, 'Wide')"));
assert.ok(source.includes("resizeTo({ width: 540, height: 640 }, 'Compact')"));
assert.ok(!source.includes('Emulation.setDeviceMetricsOverride'), 'a fake viewport cannot qualify native size evidence');
await import('./test-native-workspace-contracts.mjs');
console.log('PASS real scope parsing; 47+20 unique action cases; connected cluster IPC/matrix; Chat/Send snapshot matrix and presentation gates; Swift source contracts; layout acceptance/rejection; IPC registration/injection; disjoint CI scopes with unchanged three-minute ceilings');
