import { Button } from './components/Button';
import { useCallback, useEffect, useRef, useState } from 'react';
import mark from './assets/orka-mark.svg';
import Setup, { type LocalClusterControl } from './setup/Setup';
import { firstTestPending, retireFirstTest, useFirstTest } from './setup/first-test';
import Example, { type DocumentAction } from './designer/Example';
import Overview from './overview/Overview';
import { SAMPLE_DOCUMENT_ID } from './designer/agent-api';
import { selectedDocument } from './designer/documents-api';
import type { SetupStatus, ExamplePair } from './setup/state';
import type { SelectedPlace } from './clusters/api';
import type { ModelState } from './setup/Model';
import { workspaceRecovery } from './setup/recovery-info';
import { preflightRecovery } from './setup/preflight-state';
import { setupView } from './setup/progress';
import { runCall, type RunSnapshot } from './designer/run-api';
import { agentInfo, sameSource, type SaveState, type SourceId } from './designer/agent-api';

const pages = ['Overview', 'Designer', 'Clusters'] as const;
type Page = (typeof pages)[number];

const emptyCopy: Record<Page, { title: string; detail: string }> = {
  Overview: {
    title: 'Your agents, wherever they run',
    detail: 'Agents on this machine and your connected clusters will appear here.',
  },
  Designer: {
    title: 'Build and test your agent',
    detail: 'Your agent file, prompt, tools and tests will live here.',
  },
  Clusters: {
    title: 'Choose where agents run',
    detail: 'Your local Orka and connected clusters will appear here.',
  },
};

function PageIcon({ page }: { page: Page }) {
  return (
    <svg className="icon" viewBox="0 0 24 24" aria-hidden="true">
      {page === 'Overview' && <>
        <rect x="3" y="4" width="18" height="4.5" rx="1.5" />
        <rect x="3" y="10" width="18" height="4.5" rx="1.5" />
        <rect x="3" y="16" width="18" height="4.5" rx="1.5" />
      </>}
      {page === 'Designer' && <path d="M4 20l1-4L16 5a2.1 2.1 0 0 1 3 3L8 19l-4 1zM14 7l3 3" />}
      {page === 'Clusters' && <path d="M12 3 3 8l9 5 9-5-9-5zM3 13l9 5 9-5" />}
    </svg>
  );
}

function ComingButton({ label, reason }: { label: string; reason: string }) {
  // The wrapper makes the disabled control's reason reachable with the keyboard.
  return <span className="coming-control" tabIndex={0} role="group" aria-label={`${label}: ${reason}`} title={reason}>
    <Button type="button" kind="secondary" disabled title={reason}>{label}</Button>
    <span className="control-tip" aria-hidden="true">{reason}</span>
  </span>;
}

export default function App() {
  const [page, setPage] = useState<Page>('Clusters');
  const [place, setPlace] = useState<SelectedPlace>({ kind: 'thisComputer' });
  const remote = place.kind === 'cluster';
  const content = useRef<HTMLElement>(null);
  useEffect(() => { if (content.current) content.current.scrollTop = 0; }, [page]);
  const [exampleOpen, setExampleOpen] = useState(false);
  const [documentId, setDocumentId] = useState(SAMPLE_DOCUMENT_ID);
  const [newAgentRequest, setNewAgentRequest] = useState(0);
  const [agentsRevision, setAgentsRevision] = useState(0);
  const [persistInitialSelection, setPersistInitialSelection] = useState(false);
  const [openDocumentRequest, setOpenDocumentRequest] = useState(0);
  const openRequest = useRef(0);
  const [overviewCounts, setOverviewCounts] = useState('Reading agents…');
  const [documentAction, setDocumentAction] = useState<DocumentAction>();
  const actionSequence = useRef(0);
  const handleDocumentAction = useCallback((id: number) => setDocumentAction(current => current?.id === id ? undefined : current), []);
  const [exampleAttempt, setExampleAttempt] = useState(0);
  const [setup, setSetup] = useState<SetupStatus>();
  const [setupReadOnly, setSetupReadOnly] = useState(true);
  const workspaceAdmitted = useRef(false);
  const [editorOwned, setEditorOwned] = useState(false);
  const exampleReadOnly = setupReadOnly && !editorOwned && !(place.kind === 'cluster' && place.target);
  useEffect(() => {
    if (exampleOpen && (!setupReadOnly || place.kind === 'cluster' && place.target)) setEditorOwned(true);
  }, [exampleOpen, setupReadOnly, place]);
  const [checking, setChecking] = useState(true);
  const [welcome, setWelcome] = useState(false);
  const [model, setModel] = useState<ModelState>('idle');
  const [catalogEpoch, setCatalogEpoch] = useState(0);
  const [workspaceBusy, setWorkspaceBusy] = useState(false);
  const [workspaceStopping, setWorkspaceStopping] = useState(false);
  const localCluster = useRef<LocalClusterControl>(null);
  const [localSetupView, setLocalSetupView] = useState(false);
  const catalogContext = useRef('');
  const observeModel = useCallback((value: ModelState) => { setModel(value); setCatalogEpoch(epoch => epoch + 1); }, []);
  const observeWorkspaceAction = useCallback((pending: boolean, stopping = false) => {
    setWorkspaceBusy(pending); setWorkspaceStopping(pending && stopping);
    if (pending) setCatalogEpoch(epoch => epoch + 1);
  }, []);
  const choosePlace = useCallback((value: SelectedPlace) => { setPlace(value); setCatalogEpoch(epoch => epoch + 1); }, []);
  const [agentName, setAgentName] = useState<string>();
  const [saveState, setSaveState] = useState<SaveState>('saved');
  const [stale, setStale] = useState(false);
  const [exampleState, setExampleState] = useState<'loading' | 'loaded' | 'failed'>('loading');
  const [run, setRun] = useState<RunSnapshot>();
  const [runUnavailable, setRunUnavailable] = useState(false);
  // Setup keeps the sample's latest evidence even while Designer shows another document.
  const [sampleResult, setSampleResult] = useState<{ snapshot?: RunSnapshot }>();
  // null records an unsuccessful sample open; a retained run report cannot prove its files readable.
  const [sampleSource, setSampleSource] = useState<SourceId | null>();
  const [sampleStale, setSampleStale] = useState(false);
  const [sampleDirty, setSampleDirty] = useState(false);
  const [sampleRevision, setSampleRevision] = useState(0);
  const observeSampleRun = useCallback((snapshot: RunSnapshot | undefined) => {
    // Observed sample completion consumes held setup intent; a later unreadable poll must not re-arm it.
    if (snapshot?.state === 'complete' && snapshot.report) retireFirstTest();
    setSampleResult({ snapshot });
  }, []);
  const sampleObserved = sampleResult !== undefined;
  const [sampleCheck, setSampleCheck] = useState(0);
  useEffect(() => {
    if (!sampleObserved && sampleCheck === 0) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const observe = async () => {
      const result = await runCall('agent_run_status');
      if (cancelled) return;
      observeSampleRun(result.ok ? result.snapshot : undefined);
      timer = setTimeout(() => { void observe(); }, 500);
    };
    void observe();
    return () => { cancelled = true; clearTimeout(timer); };
  }, [sampleObserved, sampleCheck, observeSampleRun]);
  const observeRun = useCallback((snapshot: RunSnapshot | undefined, id: string) => {
    setRun(snapshot); setRunUnavailable(!snapshot);
    if (id === SAMPLE_DOCUMENT_ID) observeSampleRun(snapshot);
  }, [observeSampleRun]);
  const observeStale = useCallback((value: boolean, id: string) => {
    setStale(value);
    if (id === SAMPLE_DOCUMENT_ID) setSampleStale(value);
  }, []);
  const observeSave = useCallback((value: SaveState, id: string) => {
    setSaveState(value);
    if (id === SAMPLE_DOCUMENT_ID) setSampleDirty(value !== 'saved');
  }, []);
  const observeSetup = useCallback((status: SetupStatus | undefined) => {
    setSetup(status); setChecking(false);
    const context = `${status?.selected}/${status?.localVm}/${status?.model}`;
    if (context !== catalogContext.current) { catalogContext.current = context; setCatalogEpoch(epoch => epoch + 1); }
    // An unavailable poll must not admit discovery or Run during initial setup.
    if (status) {
      if (!status.selected) workspaceAdmitted.current = false;
      if (status.localVm === 'ready') workspaceAdmitted.current = true;
      setSetupReadOnly(status.selected === 'localVm' && !workspaceAdmitted.current);
    }
  }, []);
  const observePair = useCallback((pair: (ExamplePair & { source?: SourceId }) | undefined, id: string) => {
    setExampleState(pair ? 'loaded' : 'failed');
    setAgentName(agentInfo(pair?.agent).name);
    if (id === SAMPLE_DOCUMENT_ID) setSampleSource(pair ? pair.source : null);
  }, []);
  const cases = run?.report?.cases;
  const checksStale = stale || (run?.sourceCurrent === false && !!run.report);
  const localChecks = checksStale ? 'Tests not run since your edit' : runUnavailable ? 'Test status unavailable' : run?.cancellation === 'unconfirmed' ? 'Cancellation not confirmed' : run?.state === 'unknown' ? 'Test result unknown'
    : run?.cancellation === 'cancelling' ? 'Cancelling tests' : run?.state === 'running' ? 'Running tests…' : run?.report?.verdict === 'cancelled' ? 'Tests cancelled' : run?.report?.verdict === 'unknown' ? 'Test result unknown' : cases
    ? `${cases.filter(check => check.verdict === 'passed').length} of ${cases.length} tests passing` : undefined;
  const checks = remote && localChecks ? `This computer: ${localChecks}` : localChecks;
  const firstTest = useFirstTest({
    ready: setup?.selected === 'localVm' && setup.localVm === 'ready' && model === 'connected' && !workspaceBusy && !remote,
    canOpen: page === 'Clusters' && localSetupView && setup?.localVm === 'ready' && !workspaceBusy && !remote,
    sampleDirty,
    onOpen: () => openDesigner(SAMPLE_DOCUMENT_ID),
    onPrepared: () => setSampleRevision(value => value + 1),
    onSnapshot: observeSampleRun,
  });
  const sample = sampleResult?.snapshot;
  const firstCheck = sampleSource === null ? 'unknown' : firstTest === 'running' ? 'running' : sampleResult ? sampleStale || (sameSource(sampleSource, sample?.source) === false && !!sample?.report) ? 'stale'
    : !sample || sample.state === 'unknown' ? 'unknown' : sample.state === 'running' ? 'running' : sample.state === 'complete' ? sample.report?.verdict === 'cancelled' ? 'unknown' : sample.report?.verdict : undefined
    : firstTest;
  // Tab, footer and the Setup card share this derivation. Native selection, not the default radio, names the host.
  const initialAutomatic = firstTest === 'running' || (model === 'connected' && firstTestPending() && (!sample || sample.state === 'idle'));
  const firstTestResult = sampleSource !== null && sample?.report ? { failed: sample.report.cases.filter(check => check.verdict === 'failed').length } : undefined;
  const view = setupView(setup, { checking, stopping: workspaceStopping, model, check: firstCheck, initialAutomatic });
  const { line: setupLine, detail, tone: hostTone } = view;
  const workspaceReady = setup?.localVm === 'ready';
  const placeName = remote ? place.target?.context_name ?? 'Choose a cluster'
    : { local: 'This computer', none: 'No cluster selected', checking: 'Checking…', unavailable: 'Status unavailable' }[view.place];
  const hostDetail = setup?.localVm === 'failed' ? (setup.preflightFailure ? preflightRecovery(setup.preflightFailure) : workspaceRecovery(setup.workspaceFailure)).summary : detail;
  const failedChecks = cases?.filter(check => check.verdict === 'failed').length ?? 0;
  const agentStatus = checksStale ? 'tests need running' : runUnavailable ? 'test status unavailable' : run?.cancellation === 'unconfirmed' ? 'cancellation not confirmed' : run?.state === 'unknown' ? 'test result unknown'
    : run?.cancellation === 'cancelling' ? 'cancelling tests'
    : run?.state === 'running' ? 'running tests'
    : run?.report?.verdict === 'cancelled' ? 'tests cancelled'
    : run?.report?.verdict === 'unknown' ? 'test result unknown'
    : failedChecks ? `${failedChecks} ${failedChecks === 1 ? 'test' : 'tests'} failing`
    : cases?.some(check => check.verdict === 'unknown') ? 'test result unknown'
    : cases?.some(check => check.verdict === 'cancelled') ? 'tests cancelled'
    : cases ? 'tests passing' : 'ready to edit';
  const tabStatus: Record<Page, string> = {
    Overview: overviewCounts,
    Designer: agentName ? `${agentName}, ${remote ? 'tests on your cluster' : agentStatus}` : !exampleOpen ? 'No agent open'
      : exampleState === 'failed' ? 'Example unavailable' : exampleState === 'loaded' ? 'Agent file' : 'Opening example…',
    Clusters: remote ? place.target ? 'Access confirmed' : 'Choose a cluster' : welcome ? 'Start here' : setupLine,
  };
  const tabTone = remote || exampleReadOnly ? 'not-started' : stale || (run?.sourceCurrent === false && !!run.report) ? 'stale' : run?.state === 'running' ? 'running' : run?.cancellation === 'unconfirmed' ? 'needs-you' : run?.report?.verdict === 'failed' ? 'failed' : run?.report?.verdict === 'passed' ? 'passed' : 'not-started';
  const copy = emptyCopy[page];
  const openDesigner = (id?: string, create = false) => {
    setDocumentAction(undefined);
    const request = ++openRequest.current;
    setPage('Designer');
    const open = (selected: string) => {
      if (request !== openRequest.current) return;
      const remount = exampleState === 'failed' && saveState === 'saved';
      if (remount) { setExampleAttempt(value => value + 1); setExampleState('loading'); }
      if (!exampleOpen || remount) setPersistInitialSelection(id !== undefined);
      else if (id !== undefined) setOpenDocumentRequest(value => value + 1);
      setDocumentId(selected); setExampleOpen(true);
      if (create) setNewAgentRequest(value => value + 1);
    };
    if (id || exampleOpen) open(id ?? documentId);
    else void selectedDocument().then(selected => open(selected ?? SAMPLE_DOCUMENT_ID));
  };
  const openOverviewDocument = (id: string, action: 'open' | 'chat' | 'run') => {
    if (remote) choosePlace({ kind: 'thisComputer' });
    openDesigner(id);
    setDocumentAction({ id: ++actionSequence.current, documentId: id, action });
  };
  const footer = [
    remote ? place.target ? `Cluster: ${place.target.context_name}` : 'Choose a cluster' : welcome ? 'Nothing is set up yet' : setup?.localVm === 'failed' || setup?.model === 'locked' && setup?.localVm === 'ready' ? hostDetail : setupLine,
    exampleState === 'loaded' ? { saved: 'Saved', dirty: 'Unsaved changes', saving: 'Saving…', error: 'Not saved' }[saveState] : undefined,
  ].filter(Boolean);
  const checkFirstTest = () => {
    setSampleCheck(value => value + 1);
    // Retry an unreadable source through Designer's existing guarded sample-open path.
    if (sampleSource === null) openDesigner(SAMPLE_DOCUMENT_ID);
  };
  return (
    <div className="app-shell">
      <header className="tab-bar">
        <span className="brand"><img src={mark} width="32" height="32" alt="Orka" /><b>Bridge</b></span>
        <nav className="page-nav" aria-label="Main">
          {pages.map(name => (
            <button key={name} className="page-tab" type="button"
              aria-label={name} aria-describedby={`tab-status-${name}`}
              aria-current={page === name ? 'page' : undefined}
              onClick={() => {
                setDocumentAction(undefined);
                if (name === 'Designer' && !exampleOpen) openDesigner();
                else setPage(name);
              }}>
              <span className={`icon-tile tile-${name.toLowerCase()}`}><PageIcon page={name} /></span>
              <span className="tab-copy"><b>{name}</b><small id={`tab-status-${name}`} title={tabStatus[name]}>{name === 'Designer' && agentName ? <><span className="tab-agent-name">{agentName}, </span><span className={`tab-status-word status-${tabTone}`}>{tabStatus[name].slice(agentName.length + 2)}</span></> : tabStatus[name]}</small></span>
            </button>
          ))}
        </nav>
        <div className="chrome-tools">
          <span className="host-chip" title={remote ? placeName : hostDetail}><span className={`live-dot ${remote ? place.target ? 'ready' : 'wait' : hostTone}`} aria-hidden="true" /><span className="host-label">{placeName}</span><span className="sr-only">: {remote ? place.target ? 'Access confirmed' : 'Not connected' : hostDetail}</span></span>
        </div>
      </header>
      <main ref={content} className="page-content" aria-labelledby="page-title">
        {page === 'Designer' && !exampleOpen && <div className="page-heading">
          <h1 id="page-title">{page}</h1>
          {page === 'Designer' && <div className="heading-actions">
            <Button type="button" kind="secondary" onClick={() => openDesigner(undefined, true)}>New agent</Button>
            <ComingButton label="Send to a cluster" reason="Open your agent before sending" />
          </div>}
        </div>}
        <div hidden={page !== 'Clusters'}>
          <Setup ref={localCluster} place={place} onPlace={choosePlace} onLocalView={setLocalSetupView} onWorkspaceAction={observeWorkspaceAction} active={page === 'Clusters'} firstCheck={firstCheck} firstTestResult={firstTestResult} initialAutomatic={initialAutomatic} onCheckFirstTest={checkFirstTest} onStarting={() => setSetupReadOnly(true)} onStatus={observeSetup} onModelState={observeModel} onWelcome={setWelcome}
            onOpenExample={() => openDesigner(SAMPLE_DOCUMENT_ID)} onOpenAgent={() => openDesigner()} />
        </div>
        {exampleOpen && <div hidden={page !== 'Designer'}><Example key={exampleAttempt} sampleRevision={sampleRevision} actionRequest={documentAction} onActionHandled={handleDocumentAction} agentsRevision={agentsRevision} onRegistered={() => setAgentsRevision(value => value + 1)} documentId={documentId} persistInitialSelection={persistInitialSelection} openDocumentRequest={openDocumentRequest} newAgentRequest={newAgentRequest} onNewAgentHandled={() => setNewAgentRequest(0)} onDocument={setDocumentId} remote={remote} catalogKey={!remote && !workspaceBusy && setup?.selected === 'localVm' && workspaceReady && setup?.model !== 'locked' && model === 'connected' ? catalogEpoch : undefined} place={place} onChooseCluster={() => setPage('Clusters')} onStopLocalCluster={() => { setPage('Clusters'); localCluster.current?.stop(); }} setupPending={exampleReadOnly} active={page === 'Designer'} onPair={observePair} onRun={observeRun} onSaveState={observeSave} onStale={observeStale} /></div>}
        <Overview onNewAgent={() => openDesigner(undefined, true)} active={page === 'Overview'} runBusy={firstTest === 'running' || !!run?.busy} activeDocumentId={firstTest === 'running' ? SAMPLE_DOCUMENT_ID : run?.busy?.documentId} workspaceKey={`${setup?.localVm ?? 'unknown'}/${workspaceBusy}`} onCounts={setOverviewCounts}
          onOpenDocument={id => openOverviewDocument(id, 'open')}
          onChatDocument={id => openOverviewDocument(id, 'chat')}
          onRunDocument={id => openOverviewDocument(id, 'run')}
          onOpenExample={() => { if (remote) choosePlace({ kind: 'thisComputer' }); openDesigner(SAMPLE_DOCUMENT_ID); }}
          onChooseWorkspace={() => setPage('Clusters')} />
        {page === 'Designer' && !exampleOpen && <section className="card empty-state">
          <span className="icon-tile empty-icon"><PageIcon page={page} /></span>
          <div>
            <h2>{copy.title}</h2>
            <p className="empty-status">Nothing is connected yet.</p>
            <p className="muted">{copy.detail}</p>
          </div>
        </section>}
      </main>
      <footer className="status-bar">
        {footer.map((part, index) => <span key={index}>{index > 0 && <span aria-hidden="true"> · </span>}<span aria-live={index > 0 ? 'polite' : undefined}>{part}</span></span>)}
        {checks && <span className="checks-status">{checks}</span>}
        <span className="sr-only" role="status" aria-atomic="true">{[
          page !== 'Clusters' || !localSetupView ? `Local cluster: ${hostDetail === setupLine ? setupLine : `${setupLine} · ${hostDetail}`}` : undefined,
          page !== 'Designer' && checks ? `Checks: ${checks}` : undefined,
        ].filter(Boolean).join(' · ')}</span>
      </footer>
    </div>
  );
}
