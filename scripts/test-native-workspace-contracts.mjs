// Executable contracts for synthetic renderer drivers; not native rendering evidence.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const read = async name => (await readFile(new URL(name, import.meta.url), 'utf8')).replaceAll('\r\n', '\n');
const source = await read('./native-recovery-windows.mjs');
const refusalExpression = source.match(/const refusal = await evaluate\(`([^`]+)`\);/)[1];
const retry = { textContent: 'Try again', disabled: true, title: 'Open the matching Bridge version.' };
const refusalCard = { innerText: 'Open the matching Bridge version.', getClientRects: () => [1], querySelectorAll: () => [retry] };
const refusal = Function('document', `return ${refusalExpression}`)({ querySelectorAll: selector => selector === '.workspace-failure' ? [refusalCard] : [retry] });
assert.equal(refusal.disabled, true, 'first-setup image refusal must find its actual disabled Try again control');
assert.equal(refusal.title, 'Open the matching Bridge version.');
const recoverySource = await read('../app/src/setup/recovery-info.ts');
const causes = Function(`return ${recoverySource.match(/const causes = (\[[^\n]+\]) as const/)[1]}`)();
const captureSetup = source.slice(source.indexOf('  async function captureSetup('), source.indexOf('  const actionFixtures'));
let setupInjection;
const setupStop = new Error('setup injection captured');
const setupCapture = vm.runInNewContext(`(${captureSetup})`, {
  frameTree: { frame: { id: 'fixture' } }, samplePair: { agent: '', tests: '' },
  call: async () => ({ executionContextId: 1 }), evaluate: async script => { setupInjection = script; throw setupStop; },
});
await assert.rejects(setupCapture('dark', 'setup-fit', ['setup-transfer', 'installing', null, false, 'Downloading']), error => error === setupStop);
const setupValues = new Map([['bridge.firstTest.v1', 'attempted']]);
vm.runInNewContext(setupInjection, { window: {}, localStorage: {
  setItem: (key, value) => setupValues.set(key, value), removeItem: key => setupValues.delete(key),
}, document: { getElementById: () => ({ replaceWith() {} }), createElement: () => ({}) } });
assert.equal(setupValues.has('bridge.firstTest.v1'), false, 'fresh setup must not inherit returning intent from earlier captures');
const captureActions = source.slice(source.indexOf('  async function captureActions('), source.indexOf('  // Overview renderer fixtures:'));
async function fixture(name) {
  let injection;
  const stop = new Error('injection captured');
  const capture = vm.runInNewContext(`(${captureActions})`, {
    pair: { agent: '', tests: '' }, replySavedAgent: '', GiB: 1024 ** 3, fixtureAccount: 'synthetic-user',
    call: async () => ({ frameTree: { frame: { id: 'fixture' } }, executionContextId: 1 }),
    until: async () => {},
    evaluate: async script => { injection = script; throw stop; },
  });
  await assert.rejects(capture('light', 'wide', name), error => error === stop);
  const values = new Map();
  const window = { dispatchEvent() {} };
  vm.runInNewContext(injection, { window, Event: class {}, localStorage: {
    setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key),
  }, document: { getElementById: () => ({ replaceWith() {} }), createElement: () => ({}) } });
  return { window, values, invoke: window.__TAURI_INTERNALS__.invoke };
}
for (const name of ['local-stopped', 'local-stop-failed', 'local-stop-lost-reply', 'designer-waiting']) {
  const { window, values, invoke } = await fixture(name);
  assert.equal(values.get('bridge.firstTest.v1'), 'attempted', `${name}: saved returning intent`);
  window.__bridgeSettleWorkspace('starting');
  let status = await invoke('setup_status');
  assert.equal(status.selected, 'localVm');
  assert.equal(status.localVm, 'starting');
  assert.equal(status.workspaceProgress.elapsedMs, 18000);
  for (const phase of ['ready', 'stopped-unexpectedly', 'failed', 'stop-failed', 'closed']) {
    window.__bridgeSettleWorkspace(phase);
    status = await invoke('setup_status');
    assert.equal(status.localVm, ['stop-failed', 'stopped-unexpectedly'].includes(phase) ? 'failed' : phase);
    if (status.workspaceFailure) assert.ok(causes.includes(status.workspaceFailure.cause), 'fixture must use the actual native failure schema');
    if (phase === 'stop-failed') assert.equal(status.workspaceFailure.operation, 'stop');
    if (phase === 'stopped-unexpectedly') {
      assert.equal(status.workspaceFailure.cause, 'unknown');
      assert.match(status.localVmError, /stopped unexpectedly/);
    }
  }
  assert.equal(window.__bridgeStops, 0, 'state capture must not click removed workspace Stop');
  assert.equal(window.__bridgeLocalSelections, 0);
  await invoke('quit_bridge');
  assert.equal(window.__bridgeQuits, 1, 'Quit escape must use terminal native IPC');
}
for (const name of ['first-test-running', 'ai-connected']) {
  const { values } = await fixture(name);
  assert.equal(values.has('bridge.firstTest.v1'), false, 'first setup intent must not become returning');
}
const start = source.indexOf('  function assertWorkspacePresentation(');
assert.ok(start >= 0, 'workspace presentation acceptance gate missing');
const gate = Function(`${source.slice(start, source.indexOf('  async function captureWorkspace(', start))}; return assertWorkspacePresentation;`)();
const probe = { heading: 'This computer', pill: 'Ready', text: 'Agents Files', buttons: [], primary: [], firstTest: 'attempted', bar: false, choices: false, runs: 0, grid: true, detailsOpen: false, width: 540, height: 640 };
gate('ready', probe, 'compact');
gate('starting-after-quit-synthetic', { ...probe, pill: 'Starting', text: 'Agents Files 0:18 so far' }, 'compact');
gate('stop-failed', { ...probe, pill: "Couldn't stop", text: "Agents Files The workspace didn't stop.", buttons: ['Quit Bridge'], primary: ['Quit Bridge'] }, 'compact');
for (const mutation of [{ buttons: ['Stop'] }, { buttons: ['Start'] }, { grid: false }, { firstTest: undefined }, { text: 'Files only' }, { detailsOpen: true }]) {
  assert.throws(() => gate('ready', { ...probe, ...mutation }, 'compact'));
}
assert.throws(() => gate('starting-after-quit-synthetic', { ...probe, pill: 'Starting' }, 'compact'));
assert.throws(() => gate('stop-failed', { ...probe, text: "The workspace didn't stop.", buttons: ['Restart workspace'], primary: ['Restart workspace'] }, 'compact'));
for (const name of ['native-lifecycle-macos.swift', 'native-lifecycle-windows.ps1']) {
  const driver = await read(`./${name}`);
  assert.ok(driver.includes('idle-close-exited'), `${name}: idle Close evidence missing`);
  assert.ok(!driver.includes('Stop the local cluster and quit'), `${name}: obsolete native menu label`);
  assert.ok(!driver.includes('Close quit instead of hiding'), `${name}: idle must exit, not hide`);
  assert.ok(!driver.includes('first-close-notice'), `${name}: no native Ready producer, no notice claim`);
  assert.ok(driver.includes(name.endsWith('.swift') ? 'theme == "dark" ? [9, 9, 10] : [255, 255, 255]' : "if ($script:theme -eq 'dark') { @(9, 9, 10) } else { @(255, 255, 255) }"), `${name}: Orka paper palette assertion missing`);
}
const captures = JSON.parse(await read('./native-model-captures.json'));
// Upgrade frames share the destination journey but have their own exact matrix gate.
const destinationCaptures = new Set(captures['destinations-workspace'].filter(name => !name.startsWith('local-upgrade-')));
for (const size of ['wide', 'minimum']) {
  for (const state of ['ready', 'starting-after-quit-synthetic', 'failed', 'stopped-unexpectedly', 'stop-failed', 'forced-stop', 'closed', 'image-update', 'image-unsupported']) {
    assert.ok(destinationCaptures.has(`local-workspace-${state}-${size}`), `allocated capture missing: ${state}/${size}`);
    if (['failed', 'stopped-unexpectedly', 'stop-failed', 'forced-stop'].includes(state)) assert.ok(destinationCaptures.has(`local-workspace-${state}-details-${size}`), `expanded diagnostic capture missing: ${state}/${size}`);
  }
  for (const view of ['tests', 'chat']) assert.ok(destinationCaptures.has(`local-designer-waiting-${view}-${size}`), `allocated Designer waiting capture missing: ${view}/${size}`);
}
assert.ok(destinationCaptures.has('local-workspace-inside'));
assert.equal(captures.model.length, 208, '202 incoming model frames plus six existing Overview list/evidence captures must remain intact');
assert.equal(destinationCaptures.size, 51, '18 cluster/catalog frames plus33 workspace/setup/Designer frames');
for (const size of ['minimum', 'wide']) assert.ok(destinationCaptures.has(`local-workspace-setup-forced-stop-${size}`), 'first-run forced-stop capture missing');
assert.ok(captures.model.every(name => !destinationCaptures.has(name)), 'native journeys must be disjoint');
const mac = await read('./native-model-macos.swift');
assert.ok(!mac.includes('click("Stop the workspace"'), 'Mac must not navigate via removed workspace Stop');
assert.ok(mac.includes('workspaceCapture("starting-after-quit-synthetic"'), 'Mac synthetic startup capture must be labeled');
assert.ok(mac.includes('workspacePhase("setup-forced-stop")') && mac.includes('First-run forced stop must offer Quit without retry'), 'Mac first-run forced-stop controls must be checked');
const fixtureSource = await read('../app/src-tauri/examples/model_walk.rs');
// Exercise the actual fixture capability expression and AX acceptance predicate.
// These portable checks do not claim to run Swift/AX or the full Rust fixture.
const busyArm = fixtureSource.slice(fixtureSource.indexOf('        "busy"\n'), fixtureSource.indexOf('        "refresh-pending" |'));
const restoreCapability = busyArm.match(/status\["canRestore"\] = json!\(([^;]+)\);/)?.[1];
assert.ok(restoreCapability, 'busy cancellation needs an offered restore capability to fence');
for (const key of ['cancel-requested', 'cancel-not-cancellable', 'cancel-completed', 'busy', 'busy-locked']) {
  assert.equal(Function('key', `return ${restoreCapability}`)(key), key === 'cancel-requested');
}
const restoreFence = mac.match(/require\(([^\n]+), "Busy cancellation must retain disabled restore"\)/)?.[1];
assert.ok(restoreFence, 'Mac must require Restore present AND disabled after ACK');
const checkRestore = (present, enabled) => Function('anyButton', 'enabledButton', `return ${restoreFence.replaceAll('nil', 'null')}`)(() => present, () => enabled ? {} : null);
assert.equal(checkRestore(true, false), true);
assert.equal(checkRestore(false, false), false, 'absence is not a disabled recovery capability');
assert.equal(checkRestore(true, true), false, 'busy must fence the offered recovery capability');
for (const state of ['ready', 'starting', 'failed', 'stopped-unexpectedly', 'stop-failed', 'closed']) {
  assert.ok(fixtureSource.includes(`"workspace-scene-${state}"`), `Mac explicit scene missing: ${state}`);
}
assert.ok(fixtureSource.includes("localStorage.setItem('bridge.firstTest.v1', 'attempted'); location.reload();"), 'Mac returning reload must supply retired first-test intent');
assert.ok(mac.includes('workspaceCapture("stop-failed"') && mac.includes('workspaceCapture("stopped-unexpectedly"'), 'Mac failure captures missing');
// Execute the actual isolated-world Upgrade IPC producers, not a copied DTO.
const captureUpgrade = source.slice(source.indexOf('  async function captureUpgrade('), source.indexOf('  const notes = [];'));
async function upgradeFixture(name) {
  let injection;
  const stop = new Error('upgrade injection captured');
  const capture = vm.runInNewContext(`(${captureUpgrade.replace('of variants)', 'of variants.filter(v => v[1] === selectedFixture))')})`, {
    runUpgrade: true, selectedFixture: name, frameTree: { frame: { id: 'fixture' } },
    call: async () => ({ executionContextId: 1 }),
    evaluate: async script => { injection = script; throw stop; },
  });
  await assert.rejects(capture('light', 'wide'), error => error === stop, name);
  const window = {};
  vm.runInNewContext(injection, { window, localStorage: { setItem() {}, removeItem() {} },
    document: { getElementById: () => ({ replaceWith() {} }), createElement: () => ({}) } });
  return { window, invoke: window.__TAURI_INTERNALS__.invoke };
}
const dtoKeys = 'additionalBytes,busy,canCancel,canReplace,canRestore,createdAt,credential,error,repeated,retainedAllocatedBytes,sourceReleases,state,targetRelease,transaction';
for (const name of ['first-test-running', 'first-test-passed', 'ai-connected', 'local-stopped']) {
  const { invoke } = await fixture(name);
  const status = await invoke('workspace_upgrade_status');
  assert.equal(Object.keys(status).sort().join(','), dtoKeys, `${name}: ordinary fixtures need a real no-upgrade observation`);
  assert.equal(status.state, 'none');
  assert.equal(status.canCancel, false);
}
for (const name of ['upgrade-eligible-absent', 'upgrade-busy-cancel', 'upgrade-busy-locked', 'upgrade-error-forced']) {
  const { invoke } = await upgradeFixture(name);
  const status = await invoke('workspace_upgrade_status');
  assert.equal(Object.keys(status).sort().join(','), dtoKeys, name);
  assert.equal(status.canCancel, name === 'upgrade-busy-cancel', name);
  if (name === 'upgrade-busy-locked') assert.equal(status.busy, true, 'busy is not cancellation authority');
  if (name === 'upgrade-error-forced') assert.equal(status.error, 'forcedStop');
}
for (const [name, outcome] of [['upgrade-cancel-requested', 'requested'], ['upgrade-cancel-completed', 'completed'], ['upgrade-cancel-not-cancellable', 'notCancellable']]) {
  const { invoke, window } = await upgradeFixture(name);
  assert.equal(await invoke('workspace_upgrade_cancel'), outcome, 'bare cancellation outcome');
  const status = await invoke('workspace_upgrade_status');
  assert.equal(status.busy, outcome !== 'completed', 'acknowledgement is not completion');
  assert.equal(status.canCancel, false, 'no repeat cancellation after acknowledgement');
  assert.equal(window.__bridgeUpgradeCalls.some(call => call.args?.refresh), false, 'post-cancel observation is cached only');
}
for (const name of ['upgrade-refresh-busy', 'upgrade-refresh-io']) {
  const { invoke, window } = await upgradeFixture(name);
  const before = structuredClone(await invoke('workspace_upgrade_status'));
  assert.equal(before.retainedAllocatedBytes, 1200000000);
  await assert.rejects(invoke('workspace_upgrade_status', { refresh: true }), error => name.endsWith('busy') ? error === 'refreshBusy' : error === 'Synthetic owner I/O failure');
  assert.deepEqual(structuredClone(window.__bridgeUpgradeStatus), before, 'rejection itself must not change the cached DTO');
  const after = structuredClone(await invoke('workspace_upgrade_status'));
  assert.deepEqual(after, { ...before, retainedAllocatedBytes: 1300000000 }, 'only the controlled rendered metadata acknowledgement changes; transaction/history and every other field stay exact');
  assert.equal(Object.keys(after).sort().join(','), dtoKeys);
  assert.deepEqual(structuredClone(await invoke('workspace_upgrade_status')), after, 'later cached reads retain the witness');
}
{
  const { invoke, window } = await upgradeFixture('upgrade-refresh-pending');
  const before = await invoke('workspace_upgrade_status');
  let settled = false;
  const pending = invoke('workspace_upgrade_status', { refresh: true }).then(value => { settled = true; return value; });
  await Promise.resolve();
  assert.equal(settled, false, 'manual refresh must actually remain pending');
  assert.deepEqual(await invoke('workspace_upgrade_status'), before, 'cached observation stays independent of slow owner refresh');
  window.__bridgeFinishUpgradeRefresh();
  assert.deepEqual(await pending, before);
}
{
  const { invoke } = await upgradeFixture('upgrade-forward-cancelled');
  const reply = await invoke('workspace_upgrade_retry', { transaction: '40000000-0000-4000-8000-000000000004' });
  assert.equal(Object.keys(reply).sort().join(','), 'outcome,status');
  assert.equal(reply.outcome, 'cancelled');
  assert.equal(reply.status.state, 'incomplete');
  assert.equal(reply.status.busy, false);
  await assert.rejects(invoke('workspace_upgrade_restore', { transaction: reply.status.transaction }), /Unexpected native mutation/);
}
// Capture waits must use the manual-check error rendered by Setup, not the
// API client's generic failure text: cached reads still succeed in these scenes.
const setup = await read('../app/src/setup/Setup.tsx');
const manualError = setup.match(/'([^'\n]*Could not check workspace files[^'\n]*)'/)?.[1];
assert.ok(manualError, 'actual Setup manual owner-refresh error missing');
const refreshedExpression = source.match(/const refreshedText = ([^;]+);/)?.[1];
assert.ok(refreshedExpression, 'Windows manual-refresh capture expectation missing');
const model = await read('./native-model-macos.swift');
// Drive the actual early-message call's delta and seek loop bound from a bottom
// viewport. A reversed wheel sign must fail rather than exhausting retries below.
function earlySeek(driver) {
  const helper = driver.match(/func seek\([\s\S]*?\n\}/)?.[0];
  const call = driver.match(/seek\("Remember the checksum ALPHA\.", (-?\d+)([^\n]*)/);
  assert.ok(helper && call, 'actual long-early seek and helper must exist');
  const bound = Number(helper.match(/for (?:_|attempt) in 0\.\.<(\d+)/)?.[1]);
  assert.equal(bound, 6, 'recovery must retain the existing six attempts');
  assert.match(helper, /scroll\(pixels\)/, 'misses must use the caller delta');
  let position = 1000;
  const captures = [];
  for (let attempt = 0; attempt < bound; attempt++) {
    captures.push(position);
    if (position === 0) return { captures, helper, call };
    position = Math.max(0, Math.min(1000, position - Number(call[1])));
  }
  assert.fail('long-early seek cannot recover from the bottom');
}
const earlyRecovery = earlySeek(model);
assert.deepEqual(earlyRecovery.captures, [1000, 0], 'large upward recovery must capture bottom then top');
assert.throws(() => earlySeek(model.replace(/seek\("Remember the checksum ALPHA\.", \d+/, 'seek("Remember the checksum ALPHA.", -100')), /cannot recover/);
assert.ok(earlyRecovery.call[2].includes(String.raw`diagnosticPrefix: "chat-long-early-\(size)-seek"`), 'only explicit early caller opts into per-size diagnostics');
assert.match(earlyRecovery.helper, /diagnosticPrefix: String\? = nil/);
assert.ok(earlyRecovery.helper.indexOf('if let prefix = diagnosticPrefix') > earlyRecovery.helper.indexOf('capture("local-probe", [])'));
assert.ok(earlyRecovery.helper.indexOf('if let prefix = diagnosticPrefix') < earlyRecovery.helper.indexOf('if seen.contains'), 'preserve first and recovered captures before returning');
for (const field of ['label', 'pixels', 'attempt', 'width', 'height']) {
  assert.match(earlyRecovery.helper, new RegExp('"' + field + '":'), 'trace metadata: ' + field);
}
assert.match(earlyRecovery.helper, /for ext in \["png", "txt"\]/, 'preserve existing probe pixels and OCR');
assert.ok(earlyRecovery.helper.includes(String.raw`copyItem(atPath: "\(output)/local-probe.\(ext)", toPath: "\(stem).\(ext)")`));
const stemTemplate = earlyRecovery.helper.match(/let stem = "([^"]+)"/)?.[1];
assert.ok(stemTemplate, 'diagnostic filename template missing');
const diagnosticPaths = [];
for (const size of ['wide', 'minimum']) for (let attempt = 0; attempt < 6; attempt++) {
  const values = { output: 'captures', prefix: `chat-long-early-${size}-seek`, attempt };
  diagnosticPaths.push(stemTemplate.replace(/\\\((\w+)\)/g, (_, key) => values[key]));
}
assert.equal(new Set(diagnosticPaths).size, 12, 'both viewport histories must retain every distinct attempt');
assert.equal(diagnosticPaths[0], 'captures/chat-long-early-wide-seek-attempt-0');
assert.equal((model.match(/diagnosticPrefix: "chat-long-early-/g) ?? []).length, 1, 'only early seek opts into diagnostics');
const macRefresh = model.match(/expected = state == "refresh-pending" \? "([^"]+)" : "([^"]+)"/);
assert.ok(macRefresh, 'Mac manual-refresh capture expectation missing');
const mismatches = [];
for (const state of ['refresh-busy', 'refresh-io']) {
  const windowsError = Function('name', `return ${refreshedExpression}`)(`upgrade-${state}`);
  if (windowsError !== manualError) mismatches.push(`Windows ${state}: ${windowsError}`);
  if (macRefresh[2] !== manualError) mismatches.push(`Mac ${state}: ${macRefresh[2]}`);
}
assert.deepEqual(mismatches, [], 'both native drivers must wait for the actual manual-check error');
// Execute the actual busy capture gate against both cancellation capabilities.
const busyCheck = captureUpgrade.slice(captureUpgrade.indexOf("      if (await evaluate('window.__bridgeUpgradeStatus.busy'))"), captureUpgrade.indexOf("      const calls = await evaluate('window.__bridgeUpgradeCalls')"));
assert.ok(busyCheck, 'busy capture capability gate missing');
async function checkBusy(canCancel, buttons) {
  const window = { __bridgeUpgradeStatus: { busy: true, canCancel } };
  const document = { querySelectorAll: () => buttons };
  await vm.runInNewContext(`(async () => { ${busyCheck} })()`, {
    name: canCancel ? 'upgrade-busy-cancel' : 'upgrade-busy-locked',
    evaluate: async script => Function('window', 'document', `return ${script}`)(window, document),
  });
}
await checkBusy(false, []);
await checkBusy(true, [{ textContent: 'Cancel update', disabled: false }]);
await assert.rejects(checkBusy(false, [{ textContent: 'Cancel update', disabled: false }]));
await assert.rejects(checkBusy(false, [{ textContent: 'Cancel update', disabled: true }]));
await assert.rejects(checkBusy(true, []));
await assert.rejects(checkBusy(true, [{ textContent: 'Cancel update', disabled: true }]));
await assert.rejects(checkBusy(true, [{ textContent: 'Cancel update', disabled: false }, { textContent: 'Restore previous workspace', disabled: false }]));
// A request observed before React commits must not satisfy the rendered witness wait.
const refreshCheck = captureUpgrade.slice(captureUpgrade.indexOf("      if (name.startsWith('upgrade-refresh-'))"), captureUpgrade.indexOf("      if (name.startsWith('upgrade-cancel-'))"));
async function checkRenderedRefresh(keepError, falseFreshness, automaticRefresh = false) {
  const window = { __bridgeUpgradeCalls: automaticRefresh ? [{ command: 'workspace_upgrade_status', args: { refresh: true } }] : [], dispatchEvent() {} };
  let text = 'Retained workspace: 1.2 GB allocated.';
  let committed = false;
  const run = vm.runInNewContext(`(async () => { ${refreshCheck} })()`, {
    name: 'upgrade-refresh-busy', JSON,
    Event: class {}, window: {},
    evaluate: async script => Function('document', 'window', 'Event', `return ${script}`)(
      { querySelector: () => ({ innerText: text }), querySelectorAll: () => [{ textContent: 'Check again', disabled: false, getClientRects: () => [1], click: () => { window.__bridgeUpgradeCalls.push({ command: 'workspace_upgrade_status', args: { refresh: true } }); text += manualError; } }] }, window, class {}),
    until: async (predicate, label) => {
      if (label === 'rendered cached metadata after refresh failure') {
        assert.equal(await predicate(), false, 'a request/error without the new DTO rendered cannot pass');
        text = 'Retained workspace: 1.3 GB allocated.' + (keepError ? manualError : '') + (falseFreshness ? 'Workspace files checked on request.' : '');
        committed = true;
      }
      assert.equal(await predicate(), true, label);
    },
  });
  await run;
  assert.equal(committed, true, 'consumer must wait for the rendered cached DTO, not request count');
}
await assert.rejects(checkRenderedRefresh(true, false, true), /owner refresh before Check again/);
await checkRenderedRefresh(true, false);
await assert.rejects(checkRenderedRefresh(false, false), /erased manual refresh error/);
await assert.rejects(checkRenderedRefresh(true, true), /claimed owner freshness/);
const upgradeView = await read('../app/src/setup/Upgrade.tsx');
const savedConnectionCopy = upgradeView.match(/<p>(A saved connection can be reused[^<]+)<\/p>/)?.[1];
assert.ok(savedConnectionCopy, 'read actual saved-connection copy from the renderer');
assert.ok(mac.includes(`waitForNativeText(["${savedConnectionCopy}"])`), 'eligible-credential must wait for actual saved-connection copy');
assert.ok(mac.includes(`!nativeTextExists("${savedConnectionCopy}")`), 'eligible-absent must reject saved-connection copy');
assert.ok(fixtureSource.includes('match upgrade_refresh_bytes(key, refresh, &mut ack.1)'), 'Mac IPC must use the executable producer');
// Compile and execute the real dependency-free Mac fixture metadata producer.
// This is not a native renderer/SDK run: it checks rejection/read ordering only.
const witnessProducer = fixtureSource.match(/fn upgrade_refresh_bytes\([\s\S]*?\n\}/)?.[0];
assert.ok(witnessProducer, 'Mac fixture needs an executable next-cache metadata producer');
const { execFileSync } = await import('node:child_process');
const { mkdtemp, writeFile, rm } = await import('node:fs/promises');
const { tmpdir } = await import('node:os');
const { join } = await import('node:path');
const scratch = await mkdtemp(join(tmpdir(), 'bridge-witness-'));
const executable = join(scratch, process.platform === 'win32' ? 'witness.exe' : 'witness');
try {
  const main = `${witnessProducer}\nfn main() {
    for (key, error) in [("refresh-busy", "refreshBusy"), ("refresh-io", "Synthetic owner I/O failure")] {
      let mut failed = false;
      let before = upgrade_refresh_bytes(key, false, &mut failed).unwrap();
      assert_eq!(before, 1200000000);
      assert_eq!(upgrade_refresh_bytes(key, true, &mut failed), Err(error));
      assert_eq!(before, 1200000000); // rejected request returned no replacement DTO
      assert_eq!(upgrade_refresh_bytes(key, false, &mut failed), Ok(1300000000));
      assert_eq!(upgrade_refresh_bytes(key, false, &mut failed), Ok(1300000000));
    }
    let mut failed = false;
    assert_eq!(upgrade_refresh_bytes("refresh-pending", true, &mut failed), Ok(1200000000));
    assert!(!failed);
  }`;
  await writeFile(`${scratch}/main.rs`, main);
  execFileSync('rustc', ['--edition=2021', `${scratch}/main.rs`, '-o', executable]);
  execFileSync(executable);
} finally { await rm(scratch, { recursive: true, force: true }); }
// The bare ACK can render before the next cached DTO withdraws cancellation.
// Exercise that gap in the real capture block: neither server assignment nor ACK
// text is a renderer acknowledgement of the new capability.
const cancelStart = source.indexOf("      if (name.startsWith('upgrade-cancel-')) {");
const cancelEnd = source.indexOf("      if (name === 'upgrade-forward-cancelled') {", cancelStart);
assert.ok(cancelStart >= 0 && cancelEnd > cancelStart);
for (const name of ['upgrade-cancel-requested', 'upgrade-cancel-not-cancellable']) {
  let cancelVisible = true; let observations = 0;
  const clicks = [];
  const text = name === 'upgrade-cancel-requested' ? 'Cancellation requested. Waiting for the workspace operation to stop.' : 'This workspace operation can no longer be cancelled. Check the latest status.';
  const document = { querySelector: () => ({ innerText: text }), querySelectorAll: () => cancelVisible ? [{ textContent: 'Cancel update', disabled: false }] : [] };
  await vm.runInNewContext(`(async () => { ${source.slice(cancelStart, cancelEnd)} })()`, {
    name, clickAction: async label => { clicks.push(label); }, evaluate: async script => vm.runInNewContext(script, { document }),
    until: async (predicate, label) => {
      if (label.endsWith('cancellation capability observation')) {
        assert.equal(await predicate(), false, 'ACK may precede the cached capability render');
        cancelVisible = false; observations += 1;
      }
      assert.equal(await predicate(), true, label);
    },
  });
  assert.equal(observations, 1, `${name}: capture must await the rendered capability, not just its ACK`);
  assert.deepEqual(clicks, ['Cancel update'], 'waiting must not repeat cancellation');
}
console.log('PASS native workspace driver contracts: returning intent, explicit states, Quit IPC, presentation gates, idle Close and exact Upgrade wire/refresh/cancel transitions');
