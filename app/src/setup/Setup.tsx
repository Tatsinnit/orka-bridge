import { type Ref, useCallback, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { type ClusterPlaceSnapshot, clusterPlaces, type SelectedPlace } from '../clusters/api';
import ClusterCard from '../clusters/ClusterCard';
import Places from '../clusters/Places';
import { Button } from '../components/Button';
import { Disclosure } from '../components/Disclosure';
import type { Place } from '../overview/api';
import { armFirstTest, firstTestAttempted, firstTestPending, retireFirstTest } from './first-test';
import Model, { type ModelState } from './Model';
import Preflight from './Preflight';
import { canQueueModel, setupView } from './progress';
import { setupNeeds, type WorkspaceRequirements, workspaceRequirements } from './requirements';
import { type CheckStepState, Meet, SetupProgress } from './Steps';
import { cancelLocal, type LocalVmState, type SetupStatus, selectLocal, setupStatus } from './state';
import Welcome, { hasSeenWelcome, welcomePreference } from './Welcome';
import Workspace from './Workspace';
import Upgrade, { UpgradeObservation, type UpgradeObservationState } from './Upgrade';
import { upgradeCall, type UpgradeStatus } from './upgrade-api';
import './welcome.css';
import type { RunSnapshot } from '../designer/run-api';
import FirstPass from './FirstPass';
import { workspaceRecovery } from './recovery-info';

const pending = (state: LocalVmState) => ['preparing', 'installing', 'starting', 'closing'].includes(state);
const runtimeCopy = {
  notIntegrated: 'Workspace setup is not included in this build. Nothing will be downloaded or started.',
};
export type UpgradeNotice = 'recovery' | 'required' | 'error' | 'unknown' | 'checking' | 'busy';
export type LocalClusterControl = { showPlaces: () => void };
export default function Setup({ ref, onOpenExample, onOpenAgent, onStatus, onModelState, onWelcome, onExampleName, firstCheck, firstPass, modelUnavailable, firstTestResult, initialAutomatic, onCheckFirstTest, onStarting, onWorkspaceAction, onPlace, place, onLocalView, onWorkspaceMode, active = true }: {
  ref?: Ref<LocalClusterControl>;
  onOpenExample: () => void; onOpenAgent?: () => void; active?: boolean;
  onStatus?: (status: SetupStatus | undefined) => void;
  onModelState?: (state: ModelState) => void;
  onWelcome?: (visible: boolean) => void;
  onExampleName?: (name: string | undefined) => void;
  firstCheck?: CheckStepState;
  firstPass?: RunSnapshot;
  modelUnavailable?: boolean;
  firstTestResult?: { failed: number }; initialAutomatic?: boolean; onCheckFirstTest?: () => void;
  onStarting?: () => void;
  onWorkspaceAction?: (pending: boolean, stopping?: boolean, updating?: boolean, notice?: UpgradeNotice) => void;
  onPlace?: (place: SelectedPlace) => void;
  onLocalView?: (visible: boolean) => void;
  onWorkspaceMode?: (returning: boolean) => void;
  place?: SelectedPlace;
}) {
  const [status, setStatus] = useState<SetupStatus>();
  const [upgrade, setUpgrade] = useState<UpgradeStatus>();
  const [upgradeBusy, setUpgradeBusy] = useState(false);
  const [upgradeKnown, setUpgradeKnown] = useState(false);
  const [upgradeError, setUpgradeError] = useState<string>();
  const [upgradeCheck, setUpgradeCheck] = useState(0);
  const upgradeEpoch = useRef(0);
  const manualEpoch = useRef(0);
  const manualPending = useRef(false);
  const manualFailed = useRef(false);
  const manualIdentityless = useRef(false);
  const upgradeIdentity = useRef<string | null>(undefined);
  const [upgradeObservation, setUpgradeObservation] = useState<UpgradeObservationState>({ pending: false });
  const workspaceReplaced = useRef(false);
  const ordinaryStart = useRef<string | null>(undefined);
  const ordinaryRestartEpoch = useRef(0);
  const upgradeAction = useCallback((value: boolean) => {
    // A mutation invalidates both kinds of reads, including a lost mutation reply.
    ++epoch.current; ++upgradeEpoch.current; ++manualEpoch.current;
    manualPending.current = false; manualFailed.current = false; manualIdentityless.current = false; setUpgradeObservation({ pending: false });
    ordinaryStart.current = undefined;
    setUpgradeBusy(value); setUpgradeKnown(false);
    setCheck(check => check + 1);
  }, []);
  const upgrading = upgradeBusy || !!upgrade?.busy && !!upgrade.canCancel && !upgradeObservation.pending;
  const occupied = !!upgrade?.busy && !upgrading && !upgradeObservation.pending;
  const recoveryOnly = !!upgrade && ['sourceSelected', 'sourceRetained', 'incomplete', 'restoring', 'restored', 'conflict'].includes(upgrade.state);
  const notice: UpgradeNotice | undefined = upgradeObservation.pending ? 'checking' : occupied ? 'busy' : !upgradeKnown && (status?.workspaceReplaced || !!upgrade && upgrade.state !== 'none' || firstTestPending()) ? 'unknown' : recoveryOnly ? 'recovery' : upgrade?.state === 'eligible' ? 'required' : upgrade?.error ? 'error' : undefined;
  const reportedUpgrade = useRef(false);
  const [setupError, setError] = useState<string>();
  const retryUpgradeStatus = !setupError && !!status && !status.workspaceReplaced && (status.selected === null || !!upgrade);
  const error = setupError ?? (status?.workspaceReplaced ? upgradeError : undefined);
  const [busy, setBusy] = useState(true);
  const [sending, setSending] = useState(false);
  const [pendingAction, setPendingAction] = useState<'start' | 'cancel'>();
  useEffect(() => {
    if (sending || upgrading || notice || reportedUpgrade.current) onWorkspaceAction?.(sending || upgrading || occupied || upgradeObservation.pending, sending && pendingAction !== 'start', upgrading, notice);
    reportedUpgrade.current = sending || upgrading || !!notice;
  }, [sending, pendingAction, upgrading, occupied, upgradeObservation.pending, notice, onWorkspaceAction]);
  const [workspace, setWorkspace] = useState(false);
  const initialStatus = useRef(true);
  const returningSession = useRef(firstTestAttempted());
  useEffect(() => { onWorkspaceMode?.(workspace); }, [workspace, onWorkspaceMode]);
  const mutation = useRef(false);
  const mounted = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const [modelState, setModelState] = useState<ModelState>('idle');
  const [stepChoice, setStepChoice] = useState<HTMLDivElement | null>(null);
  const [stepRecovery, setStepRecovery] = useState<HTMLDivElement | null>(null);
  const [placeRecovery, setPlaceRecovery] = useState<HTMLDivElement | null>(null);
  const observeModel = useCallback((value: ModelState) => { setModelState(value); onModelState?.(value); }, [onModelState]);
  const [check, setCheck] = useState(0);
  const [setupCheck, setSetupCheck] = useState(0);
  const [seen, setSeen] = useState(hasSeenWelcome);
  const [inside, setInside] = useState(false);
  // Navigation is not destination selection. Standalone callers retain the same contract.
  const [ownPlace, setOwnPlace] = useState<SelectedPlace>({ kind: 'thisComputer' });
  const currentPlace = place ?? ownPlace;
  const currentPlaceRef = useRef(currentPlace);
  useEffect(() => { currentPlaceRef.current = currentPlace; }, [currentPlace]);
  const cluster = currentPlace.kind === 'cluster';
  const [panel, setPanel] = useState<'places' | 'local' | 'cluster'>();
  const [payoffDismissed, setPayoffDismissed] = useState(false);
  const shownPayoff = useRef(false);
  const [providerChange, setProviderChange] = useState(0);
  const view = panel ?? (status?.selected === 'localVm' && status.localVm !== 'ready' ? 'local' : 'places');
  const displayError = error ?? (view === 'local' && retryUpgradeStatus ? upgradeError : undefined);
  useEffect(() => {
    if (!panel && status?.selected === 'localVm' && status.localVm !== 'ready') setPanel('local');
  }, [panel, status?.selected, status?.localVm]);
  useEffect(() => { onLocalView?.(view === 'local'); }, [view, onLocalView]);
  const eligiblePayoff = !!firstPass && active && view === 'local' && firstCheck === 'passed' && modelState === 'connected' && !error && !sending && status?.localVm === 'ready';
  useEffect(() => {
    if (!firstPass) return;
    if (!active || view !== 'local' || shownPayoff.current && !eligiblePayoff) setPayoffDismissed(true);
    if (eligiblePayoff) shownPayoff.current = true;
  }, [firstPass, active, view, eligiblePayoff]);
  const [entry, setEntry] = useState<{ revision: number; current?: SelectedPlace; remembered?: Place }>();
  const [clusterName, setClusterName] = useState<string>();
  const [snapshot, setSnapshot] = useState<ClusterPlaceSnapshot>();
  const [snapshotUnavailable, setSnapshotUnavailable] = useState(false);
  const snapshotEpoch = useRef(0);
  const [requirements, setRequirements] = useState<WorkspaceRequirements | null>();
  const [preflightBlocked, setPreflightBlocked] = useState(true);
  const [location, setLocation] = useState<'local' | 'cluster'>('local');
  const epoch = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const heading = useRef<HTMLHeadingElement>(null);
  const focusChoice = useRef(false);
  const welcome = !seen && !!status && !error && status.selected !== 'localVm';
  useEffect(() => { onWelcome?.(welcome); }, [welcome, onWelcome]);
  const readSnapshot = active && !welcome && (seen || !!error || !!status);
  useEffect(() => {
    if (!readSnapshot) return;
    const request = ++snapshotEpoch.current;
    setSnapshot(value => value && { ...value, selectedKey: null });
    void clusterPlaces().then(value => {
      if (request !== snapshotEpoch.current) return;
      setSnapshotUnavailable(!value);
      setSnapshot(previous => value ?? (previous && { ...previous, selectedKey: null }));
    });
    return () => { snapshotEpoch.current++; };
  }, [readSnapshot, view, currentPlace]);
  useEffect(() => {
    if (focusChoice.current && active) heading.current?.focus();
    focusChoice.current = false;
  }, [seen, status?.selected, status?.localVm, active, view]);
  useEffect(() => {
    let active = true;
    void workspaceRequirements().then(value => { if (active) setRequirements(value); });
    return () => { active = false; };
  }, []);
  useEffect(() => {
    const observe = () => {
      if (!mutation.current && document.visibilityState !== 'hidden') setCheck(value => value + 1);
    };
    window.addEventListener('focus', observe);
    document.addEventListener('visibilitychange', observe);
    return () => {
      window.removeEventListener('focus', observe);
      document.removeEventListener('visibilitychange', observe);
    };
  }, []);
  useEffect(() => {
    if (mutation.current) return;
    const request = ++epoch.current;
    void setupStatus().then(result => {
      if (request !== epoch.current) return;
      const returning = returningSession.current;
      if (result.ok) {
        if (initialStatus.current) {
          // A saved choice also exists mid-setup. Only a retired first-test intent proves a returning session.
          setWorkspace(result.status.selected === 'localVm' && result.status.localVm !== 'notIntegrated' && returning); initialStatus.current = false;
        }
        workspaceReplaced.current ||= !!result.status.workspaceReplaced;
        if (workspaceReplaced.current) result.status.workspaceReplaced = true;
        setStatus(result.status); setError(undefined);
      }
      else {
        // An unreadable observation pauses actions, not an already admitted connection.
        setError(result.message);
      }
      const waiting = result.ok && pending(result.status.localVm);
      if (result.ok && !waiting) ordinaryStart.current = undefined;
      setBusy(waiting);
      timer.current = setTimeout(() => setSetupCheck(value => value + 1), waiting ? 250 : 1000);
      onStatus?.(result.ok ? result.status : undefined);
    });
    return () => { epoch.current++; clearTimeout(timer.current); };
  }, [check, setupCheck, onStatus]);
  useEffect(() => {
    const request = ++upgradeEpoch.current;
    let timer: ReturnType<typeof setTimeout>;
    void upgradeCall('status').then(update => {
      if (request !== upgradeEpoch.current) return;
      setUpgradeKnown(update.ok);
      setUpgradeError(update.ok ? undefined : update.message);
      if (update.ok) {
        observeUpgrade(update.status);
        if (update.status.state !== 'none') retireFirstTest();
      } else if (!manualPending.current && !manualFailed.current) setUpgrade(previous => previous && previous.state !== 'none'
        ? { ...previous, state: 'conflict', canReplace: false, canRestore: false, canCancel: false, busy: false, error: 'unknown' } : previous);
      timer = setTimeout(() => setUpgradeCheck(value => value + 1), 1000);
    });
    return () => { ++upgradeEpoch.current; clearTimeout(timer); };
  }, [check, upgradeCheck]);
  function observeUpgrade(value: UpgradeStatus) {
    if (value.transaction !== ordinaryStart.current || value.canCancel || value.state !== 'fresh') ordinaryStart.current = undefined;
    // Cached observations may discover another workspace, but never certify a manual recheck.
    if ((value.transaction !== null || !manualFailed.current && !manualPending.current) && upgradeIdentity.current !== undefined && upgradeIdentity.current !== value.transaction) {
      ++manualEpoch.current; manualPending.current = false; manualFailed.current = false; manualIdentityless.current = false;
      setUpgradeObservation({ pending: false });
    }
    if ((manualFailed.current || manualPending.current) && upgradeIdentity.current && (value.transaction === null || manualIdentityless.current)) {
      // Identity loss is not another workspace or proof of repair. Keep history, not authority.
      manualIdentityless.current = true;
      setUpgrade(previous => previous && { ...previous, state: 'conflict', canReplace: false, canRestore: false, canCancel: false, busy: false, error: value.error ?? 'unknown' });
      return;
    }
    upgradeIdentity.current = value.transaction;
    setUpgrade(value);
  }
  async function checkWorkspace() {
    if (manualPending.current) return;
    const request = ++manualEpoch.current;
    manualPending.current = true;
    setUpgradeObservation(previous => ({ ...previous, pending: true, checked: false }));
    // Setup snapshots and memory observations remain independent of the owner query.
    setCheck(value => value + 1);
    const result = await upgradeCall('refresh');
    if (!mounted.current || request !== manualEpoch.current) return;
    manualPending.current = false;
    if (result.ok) {
      manualFailed.current = false; manualIdentityless.current = false;
      ++upgradeEpoch.current;
      observeUpgrade(result.status);
      setUpgradeKnown(true); setUpgradeError(undefined);
      setUpgradeObservation({ pending: false, checked: true });
      if (result.status.state !== 'none') retireFirstTest();
      setUpgradeCheck(value => value + 1);
    } else {
      manualFailed.current = true;
      setUpgradeObservation({ pending: false, error: 'Could not check workspace files. They may still be in use or unavailable. Check again before continuing.' });
    }
  }
  const ordinaryRecovery = upgradeKnown
    ? !!upgrade && ['none', 'fresh', 'consumed'].includes(upgrade.state)
    : status?.selected === 'localVm' && !status.workspaceReplaced && !upgrade;
  // Model stays mounted from preparing through ready, so it owns its queued state; no Model means no model state.
  const modelMounted = (!status?.workspaceReplaced || upgradeKnown && !!upgrade) && (!upgrade || ['none', 'fresh', 'consumed'].includes(upgrade.state)) && !upgradeBusy && !upgrade?.busy && upgrade?.credential !== 'attention' && upgrade?.credential !== 'eligible' && status?.selected === 'localVm' && (status.localVm === 'ready' || (!status.workspaceReplaced && canQueueModel(status)));
  useEffect(() => { if (!modelMounted) observeModel('idle'); }, [modelMounted, observeModel]);

  function start() {
    try { localStorage.setItem(welcomePreference, '1'); } catch { /* A presentation preference must not block setup. */ }
    focusChoice.current = true;
    setSeen(true);
  }
  function publishPlace(value: SelectedPlace) { setOwnPlace(value); onPlace?.(value); }
  function chooseCluster(item?: Place, selected = false) {
    retireFirstTest();
    start(); setPanel('cluster'); setClusterName(item?.context ?? (selected && currentPlace.kind === 'cluster' ? currentPlace.target?.context_name : undefined));
    setEntry(previous => ({ revision: (previous?.revision ?? 0) + 1, current: selected ? currentPlace : undefined, remembered: item }));
  }
  function chooseComputer() { publishPlace({ kind: 'thisComputer' }); }
  function openExample() { if (firstPass) setPayoffDismissed(true); chooseComputer(); onOpenExample(); }
  function back() { focusChoice.current = true; setPanel('places'); }
  function openLocal() { focusChoice.current = true; if (status?.selected === 'localVm' && status.localVm !== 'notIntegrated' && firstCheck && !firstTestPending() && (!firstPass || payoffDismissed)) setWorkspace(true); setLocation('local'); setPreflightBlocked(true); setPanel('local'); }
  function removed(key: string, targetId: string | undefined, navigate: boolean) {
    const current = currentPlaceRef.current;
    if (targetId && current.kind === 'cluster' && current.target?.id === targetId) publishPlace({ kind: 'cluster' });
    snapshotEpoch.current++;
    setSnapshot(value => value && { ...value, places: value.places.filter(p => p.key !== key), selectedKey: value.selectedKey === key ? null : value.selectedKey });
    if (navigate) back();
  }
  // Only an explicit ordinary start of this Fresh transaction owns ordinary Cancel.
  // Observed Version startup/closing, and UI-owned upgrade mutations, stay fenced.
  const ordinaryStarting = ordinaryStart.current !== undefined && upgradeKnown && upgrade?.state === 'fresh'
    && upgrade.transaction === ordinaryStart.current && !upgrade.canCancel && !!status && ['preparing', 'installing', 'starting'].includes(status.localVm);
  async function chooseLocal(action: 'start' | 'cancel' = 'start') {
    if (mutation.current || upgradeBusy || upgrade?.busy && !(action === 'cancel' && ordinaryStarting) || !status || (action !== 'cancel' && busy)) return;
    if (action === 'start' && !ordinaryRecovery) return;
    if (action === 'start' && status.selected === 'localVm' && !['cancelled', 'closed', 'failed'].includes(status.localVm)) return;
    // Observation must not invalidate an explicit selection awaiting its reply.
    const firstStart = action === 'start' && status.selected !== 'localVm';
    if (firstStart) armFirstTest();
    clearTimeout(timer.current);
    ++epoch.current;
    mutation.current = true;
    onWorkspaceAction?.(true, action !== 'start');
    if (action === 'start') { chooseComputer(); if (!workspace) onStarting?.(); }
    setBusy(true); setSending(true); setPendingAction(action); setError(undefined);
    const result = await (action === 'start' ? selectLocal() : cancelLocal());
    mutation.current = false;
    if (!mounted.current) return;
    onWorkspaceAction?.(false);
    setSending(false); setPendingAction(undefined);
    if (result.ok) {
      if (action === 'start') ++ordinaryRestartEpoch.current;
      focusChoice.current = true; setStatus(result.status);
    }
    else { setError(result.message); }
    onStatus?.(result.ok ? result.status : undefined);
    const waiting = result.ok && pending(result.status.localVm);
    ordinaryStart.current = action === 'start' && waiting && upgrade?.state === 'fresh' ? upgrade.transaction : undefined;
    setBusy(waiting);
    if (result.ok) setCheck(value => value + 1);
    else timer.current = setTimeout(() => setCheck(value => value + 1), 1000);
  }

  const changing = sending || upgradeBusy || !!upgrade?.busy || upgradeObservation.pending;
  const upgradePanel = upgrade && upgrade.state !== 'none' ? <Upgrade status={upgrade} ordinaryRestartEpoch={ordinaryRestartEpoch.current} observation={upgradeObservation} canUseCredential={status?.localVm === 'ready' || upgrade.state === 'consumed'} disabled={sending || (!!status && pending(status.localVm)) || !!error || !upgradeKnown} onBusy={upgradeAction} onChange={value => { observeUpgrade(value); setCheck(check => check + 1); }} onCheck={() => { void checkWorkspace(); }} /> : undefined;
  const continueAction = <Button type="button" kind="primary" aria-describedby={location === 'local' && requirements ? 'preflight-reason' : undefined} disabled={busy || !status || !!error || (location === 'local' && (changing || !ordinaryRecovery || status.localVm !== 'notIntegrated' && requirements !== null && preflightBlocked))} onClick={() => { if (location === 'cluster') chooseCluster(); else void chooseLocal(); }}>Continue</Button>;
  useImperativeHandle(ref, () => ({ showPlaces() { if (view !== 'places') back(); } }));
  const titleId = active ? 'page-title' : undefined;
  if (!status && !error) return <><h1 id={titleId}>Clusters</h1><p role="status">Checking the workspace…</p></>;
  if (welcome && status) return <Welcome titleId={titleId} onStart={start} onCluster={() => chooseCluster()} unavailable={status.localVm === 'notIntegrated' ? runtimeCopy.notIntegrated : undefined} requirements={requirements} />;
  const selected = status?.selected === 'localVm';
  const modelRecovery = ['failed', 'resetFailed', 'removalFailed', 'resetting'].includes(modelState);
  const aiHeadings: Partial<Record<ModelState, string>> = {
    resetFailed: "The AI connection couldn't be reset",
    removalFailed: "The saved sign-in couldn't be removed",
    failed: "The AI couldn't connect",
    resetting: 'Resetting the AI connection',
    connected: 'Run your first agent',
  };
  const aiHeading = aiHeadings[modelState] ?? 'Connect the AI';
  const presentation = setupView(error ? undefined : status, { stopping: sending && pendingAction !== 'start', starting: sending && pendingAction === 'start', workspace, model: modelState, check: firstCheck, initialAutomatic });
  const upgradeAttention = !error && (upgrading || !!notice && (notice !== 'unknown' || status?.workspaceReplaced));
  const upgradeLine = upgrading ? 'Updating workspace' : upgradeObservation.pending ? 'Checking workspace files' : occupied ? 'Workspace is busy' : notice === 'recovery' ? 'Workspace recovery needs attention' : 'Workspace update needs attention';
  const placesView = upgradeAttention ? { ...presentation, line: upgradeLine, detail: 'Your agent files are kept.', status: upgrading || occupied || upgradeObservation.pending ? 'running' as const : 'needs-you' as const, done: false, action: 'Manage' } : presentation;
  const showPayoff = !workspace && !payoffDismissed && eligiblePayoff;
  const localStage = status?.localVm === 'ready' ? aiHeading : status?.localVm === 'failed'
    ? status.preflightFailure ? 'Check this computer' : workspaceRecovery(status.workspaceFailure).heading : 'Getting your workspace ready';
  return <div className="places-workspace">
    {view === 'cluster' && <Button type="button" kind="quiet" className="places-back" onClick={back}>Back to clusters</Button>}
    <div className="workspace-heading places-heading" hidden={!!showPayoff}>
      <h1 ref={heading} tabIndex={-1} id={showPayoff ? undefined : titleId}>{view === 'places' ? 'Clusters' : view === 'cluster' ? clusterName ? <code>{clusterName}</code> : entry?.remembered ? 'Use a cluster' : 'Add a cluster' : workspace || selected ? 'This computer' : 'Where should your agents run?'}</h1>
      {view === 'local' && !workspace && selected && !presentation.done && <p className="muted">{localStage}</p>}
      {view === 'places' && <p className="muted">Choose where agents run</p>}
      {view === 'places' && <Button type="button" kind="secondary" onClick={() => chooseCluster()}>Add a cluster</Button>}
    </div>
    {view === 'places' && <Places modelRecovery={modelRecovery} recoveryRef={setPlaceRecovery} snapshot={snapshot} unavailable={snapshotUnavailable} status={error ? undefined : status} view={placesView} place={currentPlace} onLocal={openLocal} onCluster={chooseCluster} />}
    {showPayoff && firstPass && <FirstPass snapshot={firstPass} titleId={titleId} onOpen={openExample} />}
    <div hidden={view !== 'local' || !!showPayoff}>
      {cluster && <Button type="button" kind="secondary" onClick={chooseComputer}>Use this computer</Button>}
      <div className={workspace ? 'setup workspace-setup-grid' : selected ? `setup workspace-setup-grid${status?.localVm === 'failed' || modelRecovery ? ' workspace-setup-failed' : ''}` : 'workspace-choice local-preflight'}>
        {!selected && !workspace && <>
          <section className="card">
            <p className="workspace-choice-step muted">Step 1 of 4</p>
            <fieldset className="workspace-options">
              <legend className="workspace-sr-only">Where agents run</legend>
              <div className={`workspace-option${location === 'local' ? ' workspace-selected' : ''}`}>
                <input id="workspace-local" name="workspace-location" type="radio" value="local" checked={location === 'local'} onChange={() => { setPreflightBlocked(true); setLocation('local'); }} />
                <label htmlFor="workspace-local">On this computer</label><span className="chip">Recommended</span>
                <div className="workspace-option-detail">
                  <p>{requirements ? setupNeeds(requirements) : requirements === undefined ? "Reading this build's requirements…" : 'Requirements are not available yet for this build.'}</p>
                  {location === 'local' && view === 'local' && !error && status && requirements && status.localVm !== 'notIntegrated' && <Preflight requiredMemoryBytes={requirements.memoryBytes} onBlocked={setPreflightBlocked} onCluster={() => { setLocation('cluster'); document.getElementById('workspace-cluster')?.focus(); }} />}
                  <Disclosure label="What's inside" open={inside} controls="workspace-inside" onOpenChange={setInside}>
                    <div id="workspace-inside" hidden={!inside}>
                      <p>The workspace includes Orka and the tools your agents use. Bridge checks the download before starting it.</p>
                    </div>
                  </Disclosure>
                </div>
              </div>
              <div className={`workspace-option${location === 'cluster' ? ' workspace-selected' : ''}`}>
                <input id="workspace-cluster" name="workspace-location" type="radio" value="cluster" checked={location === 'cluster'} onChange={() => setLocation('cluster')} />
                <label htmlFor="workspace-cluster">On a cluster you already have</label>
                <div className="workspace-option-detail"><p>Use Orka on a Kubernetes cluster you already have.</p></div>
              </div>
            </fieldset>
          <div className="workspace-step-actions">{continueAction}</div>
          {location === 'local' && status?.localVm === 'notIntegrated' && <p className="muted">{runtimeCopy.notIntegrated}</p>}
          </section>
        </>}
        <div className="setup-column setup-main">
        {!workspace && !selected && upgradePanel}
        {workspace && status && <Workspace ordinaryRecovery={ordinaryRecovery} updating={upgrading} updateBlocked={!!notice && (notice !== 'unknown' || !!status.workspaceReplaced)} upgrade={upgradePanel} status={status} busy={changing || recoveryOnly} stopping={sending && pendingAction !== 'start'} starting={sending && pendingAction === 'start'} error={error} onStart={() => { void chooseLocal(); }} active={active && view === 'local'} />}
        {!workspace && selected && status && <SetupProgress ordinaryRecovery={ordinaryRecovery} updating={upgrading} updateBlocked={!!notice && (notice !== 'unknown' || !!status.workspaceReplaced)} upgrade={upgradePanel} modelChoiceRef={setStepChoice} modelUnavailable={modelUnavailable} onChooseProvider={() => setProviderChange(value => value + 1)} modelRecoveryRef={setStepRecovery} status={status} firstCheck={firstCheck} firstTestResult={firstTestResult} initialAutomatic={initialAutomatic} onCheckFirstTest={onCheckFirstTest} busy={sending || upgradeBusy || !!upgrade?.busy && !ordinaryStarting || upgradeObservation.pending || recoveryOnly} stopping={sending && pendingAction !== 'start'} error={error} modelState={modelMounted ? modelState : 'idle'} onCancel={() => { void chooseLocal('cancel'); }} onRestart={() => { void chooseLocal(); }} onCluster={() => chooseCluster()} onOpenExample={openExample} />}
        </div>
        <div className="setup-column setup-side">
        {workspace && status?.localVm !== 'ready' && <section className="card"><h2>AI</h2><p className="muted">Waiting for the workspace to start.</p></section>}
        {status && modelMounted && <div className="setup-model-slot" hidden={workspace && status.localVm !== 'ready'}><Model key={upgrade?.credential === 'transferred' ? 'shared' : 'ordinary'} sharedConnection={upgrade?.credential === 'transferred'} title={workspace ? 'AI' : undefined} inline={!workspace && !status.workspaceReplaced && canQueueModel(status)} choiceTarget={workspace ? null : stepChoice} changeRequest={providerChange} onPlaceCard={view === 'places'} recoveryTarget={view === 'places' ? placeRecovery : workspace ? null : stepRecovery} enabled={status.model === 'available'} queueable={!status.workspaceReplaced && canQueueModel(status)} paused={!!error || changing} away={cluster} primary={status.localVm === 'ready'} onConnectRequest={() => { chooseComputer(); if (view === 'places') setPanel('local'); }} onState={observeModel} onSettled={() => setCheck(value => value + 1)} /></div>}
        {!workspace && selected && status && status.localVm !== 'ready' && status.localVm !== 'failed' && <Meet onRead={openExample} onName={onExampleName} />}
        </div>
      </div>
    </div>
    {entry && <ClusterCard active={active && view === 'cluster'} entry={entry} selection={currentPlace} onPlace={publishPlace} onContextName={setClusterName} onOpenAgent={onOpenAgent ?? onOpenExample} onDone={back} onRemoved={removed} />}
    {!upgradePanel && <UpgradeObservation observation={upgradeObservation} />}
    {displayError && <div role="alert"><p>{displayError}</p>
      <Button type="button" kind="secondary" className="action-spaced" disabled={(retryUpgradeStatus ? sending : busy) || upgradeBusy || upgradeObservation.pending}
        onClick={() => { if (setupError) setSetupCheck(value => value + 1); else if (retryUpgradeStatus) setUpgradeCheck(value => value + 1); else void checkWorkspace(); }}>{retryUpgradeStatus ? 'Retry workspace status' : 'Check again'}</Button>
    </div>}
    {!error && !upgradePanel && upgradeObservation.error && <Button type="button" kind="secondary" disabled={upgradeObservation.pending} onClick={() => { void checkWorkspace(); }}>Check again</Button>}
  </div>;
}
