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
import { formatRequirementGB, type WorkspaceRequirements, workspaceRequirements } from './requirements';
import { type CheckStepState, Meet, SetupProgress } from './Steps';
import { cancelLocal, type LocalVmState, type SetupStatus, selectLocal, setupStatus } from './state';
import Welcome, { hasSeenWelcome, welcomePreference } from './Welcome';
import Workspace from './Workspace';
import './welcome.css';
import type { RunSnapshot } from '../designer/run-api';
import FirstPass from './FirstPass';
import { workspaceRecovery } from './recovery-info';

const pending = (state: LocalVmState) => ['preparing', 'installing', 'starting', 'closing'].includes(state);
const runtimeCopy = {
  notIntegrated: 'Workspace setup is not included in this build. Nothing will be downloaded or started.',
};
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
  onWorkspaceAction?: (pending: boolean, stopping?: boolean) => void;
  onPlace?: (place: SelectedPlace) => void;
  onLocalView?: (visible: boolean) => void;
  onWorkspaceMode?: (returning: boolean) => void;
  place?: SelectedPlace;
}) {
  const [status, setStatus] = useState<SetupStatus>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(true);
  const [sending, setSending] = useState(false);
  const [pendingAction, setPendingAction] = useState<'start' | 'cancel'>();
  const [workspace, setWorkspace] = useState(false);
  const initialStatus = useRef(true);
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
      if (result.ok) {
        if (initialStatus.current) {
          // A saved choice also exists mid-setup. Only a retired first-test intent proves a returning session.
          setWorkspace(result.status.selected === 'localVm' && result.status.localVm !== 'notIntegrated' && firstTestAttempted()); initialStatus.current = false;
        }
        setStatus(result.status); setError(undefined);
      }
      else {
        // An unreadable observation pauses actions, not an already admitted connection.
        setError(result.message);
      }
      const waiting = result.ok && pending(result.status.localVm);
      setBusy(waiting);
      timer.current = setTimeout(() => setCheck(value => value + 1), waiting ? 250 : 1000);
      onStatus?.(result.ok ? result.status : undefined);
    });
    return () => { epoch.current++; clearTimeout(timer.current); };
  }, [check, onStatus]);
  // Model stays mounted from preparing through ready, so it owns its queued state; no Model means no model state.
  const modelMounted = status?.selected === 'localVm' && (status.localVm === 'ready' || canQueueModel(status));
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
  async function chooseLocal(action: 'start' | 'cancel' = 'start') {
    if (mutation.current || !status || (action !== 'cancel' && busy)) return;
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
    if (result.ok) { focusChoice.current = true; setStatus(result.status); }
    else { setError(result.message); }
    onStatus?.(result.ok ? result.status : undefined);
    const waiting = result.ok && pending(result.status.localVm);
    setBusy(waiting);
    if (result.ok) setCheck(value => value + 1);
    else timer.current = setTimeout(() => setCheck(value => value + 1), 1000);
  }

  const continueAction = <Button type="button" kind="primary" aria-describedby={location === 'local' && requirements ? 'preflight-reason' : undefined} disabled={busy || !status || !!error || (location === 'local' && status.localVm !== 'notIntegrated' && requirements !== null && preflightBlocked)} onClick={() => { if (location === 'cluster') chooseCluster(); else void chooseLocal(); }}>Continue</Button>;
  useImperativeHandle(ref, () => ({ showPlaces() { if (view !== 'places') back(); } }));
  const titleId = active ? 'page-title' : undefined;
  if (!status && !error) return <><h1 id={titleId}>Clusters</h1><p role="status">Checking the workspace…</p></>;
  if (welcome && status) return <Welcome titleId={titleId} onStart={start} onCluster={() => chooseCluster()} unavailable={status.localVm === 'notIntegrated' ? runtimeCopy.notIntegrated : undefined} />;
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
    {view === 'places' && <Places modelRecovery={modelRecovery} recoveryRef={setPlaceRecovery} snapshot={snapshot} unavailable={snapshotUnavailable} status={error ? undefined : status} view={presentation} place={currentPlace} onLocal={openLocal} onCluster={chooseCluster} />}
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
                  <p>{requirements ? `${requirements.downloadBytes !== null ? `Downloads about ${formatRequirementGB(requirements.downloadBytes)}. ` : ''}Needs virtualization and ${formatRequirementGB(requirements.memoryBytes)} of memory. Data disk can grow to ${formatRequirementGB(requirements.stateDiskBytes)}.` : requirements === undefined ? "Reading this build's requirements…" : 'Requirements are not available yet for this build.'}</p>
                  {location === 'local' && view === 'local' && !error && status && requirements && status.localVm !== 'notIntegrated' && <Preflight requiredMemoryBytes={requirements.memoryBytes} onBlocked={setPreflightBlocked} onCluster={() => { setLocation('cluster'); document.getElementById('workspace-cluster')?.focus(); }} />}
                  <Disclosure label="What's inside" open={inside} controls="workspace-inside" onOpenChange={setInside}>
                    <div id="workspace-inside" hidden={!inside}>
                      <p>The workspace includes Orka and the tools your agents use. Bridge checks the download before starting it.</p>
                      {requirements && <p>{`This build's image size is ${formatRequirementGB(requirements.imageBytes)} after installation, not the amount downloaded.`}</p>}
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
        {workspace && status && <Workspace status={status} busy={sending} stopping={sending && pendingAction !== 'start'} starting={sending && pendingAction === 'start'} error={error} onStart={() => { void chooseLocal(); }} active={active && view === 'local'} />}
        {!workspace && selected && status && <SetupProgress modelChoiceRef={setStepChoice} modelUnavailable={modelUnavailable} onChooseProvider={() => setProviderChange(value => value + 1)} modelRecoveryRef={setStepRecovery} status={status} firstCheck={firstCheck} firstTestResult={firstTestResult} initialAutomatic={initialAutomatic} onCheckFirstTest={onCheckFirstTest} busy={sending} stopping={sending && pendingAction !== 'start'} error={error} modelState={modelMounted ? modelState : 'idle'} onCancel={() => { void chooseLocal('cancel'); }} onRestart={() => { void chooseLocal(); }} onCluster={() => chooseCluster()} onOpenExample={openExample} />}
        </div>
        <div className="setup-column setup-side">
        {workspace && status?.localVm !== 'ready' && <section className="card"><h2>AI</h2><p className="muted">Waiting for the workspace to start.</p></section>}
        {status && modelMounted && <div className="setup-model-slot" hidden={workspace && status.localVm !== 'ready'}><Model title={workspace ? 'AI' : undefined} inline={!workspace && canQueueModel(status)} choiceTarget={workspace ? null : stepChoice} changeRequest={providerChange} onPlaceCard={view === 'places'} recoveryTarget={view === 'places' ? placeRecovery : workspace ? null : stepRecovery} enabled={status.model === 'available'} queueable={canQueueModel(status)} paused={!!error || sending} away={cluster} primary={status.localVm === 'ready'} onConnectRequest={() => { chooseComputer(); if (view === 'places') setPanel('local'); }} onState={observeModel} onSettled={() => setCheck(value => value + 1)} /></div>}
        {!workspace && selected && status && status.localVm !== 'ready' && status.localVm !== 'failed' && <Meet onRead={openExample} onName={onExampleName} />}
        </div>
      </div>
    </div>
    {entry && <ClusterCard active={active && view === 'cluster'} entry={entry} selection={currentPlace} onPlace={publishPlace} onContextName={setClusterName} onOpenAgent={onOpenAgent ?? onOpenExample} onDone={back} onRemoved={removed} />}
    {error && <div role="alert"><p>{error}</p>
      <Button type="button" kind="secondary" className="action-spaced" disabled={busy}
        onClick={() => { setBusy(true); setError(undefined); setCheck(value => value + 1); }}>Check again</Button>
    </div>}
  </div>;
}
