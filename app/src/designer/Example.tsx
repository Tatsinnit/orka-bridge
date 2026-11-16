import { isTauri } from '@tauri-apps/api/core';
import { Disclosure } from '../components/Disclosure';
import { Button } from '../components/Button';
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { SAMPLE_DOCUMENT_ID, REPO_SAVE_CONFLICT, agentCall, agentInfo, storageCopy, type AgentPair, type SaveState } from './agent-api';
import Run from './Run';
import { Details } from '../components/DesignSystem';
import { DesignerTests } from '../components/DesignerTests';
import { testDefinitions } from './test-definitions';
import Chat from './Chat';
import { readWorkspaceCatalog, type WorkspaceCatalogReply } from '../catalog';
import AgentParts from './AgentParts';
import FileEditor from './FileEditor';
import FileLocation from './FileLocation';
import NewAgent from './NewAgent';
import { listDocuments, selectDocument, type AgentEntry } from './documents-api';
import { readForm } from './agent-form';
import type { RunSnapshot } from './run-api';
import type { SelectedPlace } from '../clusters/api';
import Send, { SendEntry, SendHistory } from '../clusters/Send';

export type DocumentAction = { id: number; documentId: string; action: 'open' | 'chat' | 'run' };

export default function Example({ sampleRevision = 0, actionRequest, onActionHandled, agentsRevision = 0, onRegistered, documentId = SAMPLE_DOCUMENT_ID, persistInitialSelection = false, openDocumentRequest = 0, newAgentRequest = 0, onNewAgentHandled, onDocument, active = true, setupPending = false, remote = false, catalogPaused = false, catalogKey, place = { kind: 'thisComputer' }, onChooseCluster, onStopLocalCluster, actions, onPair, onRun, onSaveState, onStale, onSeeInOverview }: {
  actionRequest?: DocumentAction; onActionHandled?: (id: number) => void;
  sampleRevision?: number; agentsRevision?: number; onRegistered?: () => void; documentId?: string; persistInitialSelection?: boolean; openDocumentRequest?: number; newAgentRequest?: number; onNewAgentHandled?: () => void; onDocument?: (id: string) => void; active?: boolean; setupPending?: boolean; remote?: boolean; catalogPaused?: boolean; catalogKey?: number; place?: SelectedPlace; onChooseCluster?: () => void; onStopLocalCluster?: () => void; actions?: ReactNode;
  onPair?: (pair: AgentPair | undefined, documentId: string) => void; onRun?: (snapshot: RunSnapshot | undefined, documentId: string) => void;
  onSaveState?: (state: SaveState, documentId: string) => void; onStale?: (stale: boolean, documentId: string) => void; onSeeInOverview?: (sendId: string) => void;
}) {
  const [boundDocument, setBoundDocument] = useState(documentId);
  const [requestedDocument, setRequestedDocument] = useState(documentId);
  const [selectionNeeded, setSelectionNeeded] = useState(persistInitialSelection);
  const requestedNow = useRef(documentId);
  const observedDocument = useRef(documentId);
  const requestDocument = useCallback((id: string) => { requestedNow.current = id; setRequestedDocument(id); setSelectionNeeded(true); }, []);
  const [wantsCreate, setWantsCreate] = useState(newAgentRequest > 0);
  const [creating, setCreating] = useState(false);
  const [entries, setEntries] = useState<AgentEntry[]>();
  const [listAttempt, setListAttempt] = useState(0);
  const [selectionError, setSelectionError] = useState<string>();
  const [selecting, setSelecting] = useState(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { if (observedDocument.current !== documentId) { observedDocument.current = documentId; requestDocument(documentId); } }, [documentId, requestDocument]);
  useEffect(() => { if (newAgentRequest) setWantsCreate(true); }, [newAgentRequest]);
  const handledOpen = useRef(openDocumentRequest);
  useEffect(() => {
    if (!openDocumentRequest || handledOpen.current === openDocumentRequest) return;
    handledOpen.current = openDocumentRequest;
    setWantsCreate(false); setCreating(false); requestDocument(documentId);
  }, [openDocumentRequest, documentId, requestDocument]);
  const [catalogAttempt, setCatalogAttempt] = useState(0);
  const [catalog, setCatalog] = useState<{ key: string; attempt: number; reply?: WorkspaceCatalogReply }>();
  const targetId = place.kind === 'cluster' ? place.target?.id : undefined;
  const providerId = place.kind === 'cluster' ? place.providerId : undefined;
  const catalogIdentity = `${catalogKey}/${remote}/${targetId}/${providerId}`;
  const catalogEnabled = catalogKey !== undefined && !setupPending && (!remote || !!targetId && !!providerId);
  useEffect(() => {
    if (!catalogEnabled || catalogPaused || catalogKey === undefined) return;
    const controller = new AbortController();
    void readWorkspaceCatalog(controller.signal, remote && targetId && providerId ? { targetId, providerId } : undefined).then(reply => {
      if (!controller.signal.aborted) setCatalog({ key: catalogIdentity, attempt: catalogAttempt, reply });
    });
    return () => controller.abort();
  }, [catalogEnabled, catalogPaused, catalogKey, catalogIdentity, catalogAttempt, remote, targetId, providerId]);
  const currentCatalog = catalogEnabled && !catalogPaused && catalog?.key === catalogIdentity && catalog.attempt === catalogAttempt ? catalog : undefined;
  const [pair, setPair] = useState<AgentPair>();
  useEffect(() => { let mounted = true; void listDocuments().then(value => { if (mounted) setEntries(value); }); return () => { mounted = false; }; }, [boundDocument, creating, pair?.documentId, agentsRevision, listAttempt]);
  const [draft, setDraft] = useState('');
  const [error, setError] = useState<string>();
  const [saveState, setSaveState] = useState<SaveState>('saved');
  const [pendingSoul, setPendingSoul] = useState(false);
  const pendingSoulCurrent = useRef(false);
  const [reloading, setReloading] = useState(false);
  const reloadAdmitted = useRef(false);
  const reloadEpoch = useRef(0);
  const [view, setView] = useState<'Agent' | 'Chat' | 'File'>('Agent');
  const [stale, setStale] = useState(false);
  const observePendingSoul = useCallback((pending: boolean) => { pendingSoulCurrent.current = pending; setPendingSoul(pending); if (pending) setStale(true); }, []);
  const editInstructions = () => { setView('Agent'); requestAnimationFrame(() => { const editor = document.querySelector<HTMLTextAreaElement>('.instructions-editor'); editor?.focus(); editor?.scrollIntoView?.({ block: 'center' }); }); };
  const [localRun, setLocalRun] = useState<RunSnapshot>();
  const observeRun = useCallback((value: RunSnapshot | undefined) => { setLocalRun(value); onRun?.(value, boundDocument); }, [onRun, boundDocument]);
  const [sendRequest, setSendRequest] = useState(0);
  const openSend = () => { setView('Agent'); setSendRequest(value => value + 1); };
  useEffect(() => {
    if (!sendRequest) return;
    // Focus only after Agent view and the Send card have been revealed.
    const frame = requestAnimationFrame(() => { const panel = document.querySelector<HTMLElement>('.send-panel'); panel?.focus(); panel?.scrollIntoView?.({ block: 'start' }); });
    return () => cancelAnimationFrame(frame);
  }, [sendRequest]);
  // One keyboard target: the failing local result, which then offers Edit instructions.
  const focusLocalResult = (testId: string) => requestAnimationFrame(() => {
    const result = [...document.querySelectorAll<HTMLElement>('.designer-tests [data-result-id]')].find(node => node.dataset.resultId === testId);
    const summary = result?.querySelector<HTMLElement>('summary');
    if (summary) summary.focus(); else editInstructions();
  });
  const latest = useRef(''); const saved = useRef(''); const busy = useRef(false);
  const confirmed = useRef<AgentPair | undefined>(undefined);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const lineEnding = useRef('\n'); const needsCommit = useRef(false);
  const [openAttempt, setOpenAttempt] = useState(0);
  useEffect(() => {
    let mounted = true;
    const request = ++reloadEpoch.current;
    reloadAdmitted.current = false; setReloading(false);
    const open = agentCall('agent_open', undefined, boundDocument);
    void open.then(value => {
      if (!mounted || request !== reloadEpoch.current) return;
      confirmed.current = value; setPair(value); setDraft(value.agent); latest.current = saved.current = value.agent;
      lineEnding.current = value.agent.includes('\r\n') ? '\r\n' : '\n'; onPair?.(value, boundDocument);
    }, () => { if (mounted && request === reloadEpoch.current) { setError('Your agent folder could not be opened. Open it again from Designer.'); onPair?.(undefined, boundDocument); } });
    return () => { mounted = false; };
  }, [onPair, boundDocument, openAttempt]);
  useEffect(() => { onSaveState?.(pendingSoul ? 'dirty' : saveState, boundDocument); }, [saveState, pendingSoul, onSaveState, boundDocument]);
  useEffect(() => { onStale?.(stale, boundDocument); }, [stale, onStale, boundDocument]);

  const flush = useCallback(async (retry = false) => {
    clearTimeout(timer.current);
    if (busy.current || pendingSoulCurrent.current) return;
    if (!retry && !needsCommit.current && latest.current === saved.current) { setSaveState('saved'); return; }
    busy.current = true; setSaveState('saving'); setError(undefined);
    try {
      do {
        const content = latest.current;
        const expectedSource = confirmed.current?.storage === 'repo' ? confirmed.current.source : undefined;
        const value = await agentCall('agent_save', content, boundDocument, expectedSource);
        if (value.agent !== content) throw Error('Save did not confirm the current file.');
        saved.current = content; confirmed.current = value; setPair(value); onPair?.(value, boundDocument);
        // Fingerprinted replies let Run distinguish older results from unsaved edits.
        if (value.source) setStale(latest.current !== content);
      } while (latest.current !== saved.current);
      needsCommit.current = false; setSaveState('saved');
    } catch (failure) { needsCommit.current = true; setSaveState('error'); setError(failure instanceof Error && failure.message === REPO_SAVE_CONFLICT ? REPO_SAVE_CONFLICT : 'Your changes could not be saved. Your draft is still here.'); }
    finally { busy.current = false; }
  }, [onPair, boundDocument]);
  useEffect(() => {
    // Resume an earlier autosave when the pending personality edit is resolved.
    if (!pendingSoul && !needsCommit.current && latest.current !== saved.current) {
      clearTimeout(timer.current); timer.current = setTimeout(() => { void flush(); }, 500);
    }
  }, [pendingSoul, flush]);
  useEffect(() => () => { clearTimeout(timer.current); reloadEpoch.current++; }, []);
  useEffect(() => {
    if (requestedDocument === boundDocument && !wantsCreate && !selectionNeeded) return;
    if (busy.current || pendingSoul || saveState === 'saving' || saveState === 'error' || selecting) return;
    if (saveState === 'dirty') { void flush(); return; }
    if (wantsCreate) { setCreating(true); setWantsCreate(false); onNewAgentHandled?.(); return; }
    // Persist only after the current draft is safe. The mounted editor survives failures.
    const next = requestedDocument;
    setSelecting(true);
    void selectDocument(next).then(() => {
      if (!mounted.current || requestedNow.current !== next) return;
      // The editor stays writable during IPC. Re-admit after any newer edit is saved.
      if (busy.current || needsCommit.current || pendingSoulCurrent.current || latest.current !== saved.current) return;
      setSelectionNeeded(false); setSelectionError(undefined);
      // Explicit Open (including this same file) is distinct from returning to the tab.
      setView('Agent');
      if (next === boundDocument) return;
      reloadEpoch.current++; reloadAdmitted.current = false; setReloading(false);
      confirmed.current = undefined; setPair(undefined); setDraft(''); latest.current = saved.current = '';
      setLocalRun(undefined); setStale(false); setError(undefined); setSelectionError(undefined);
      onPair?.(undefined, next); onRun?.(undefined, next); setBoundDocument(next); onDocument?.(next);
    }, () => {
      if (!mounted.current || requestedNow.current !== next) return;
      setSelectionError('Your agent selection could not be saved. Try choosing it again.');
      requestedNow.current = observedDocument.current = boundDocument;
      setRequestedDocument(boundDocument); setSelectionNeeded(false); onDocument?.(boundDocument);
    })
      .finally(() => { if (mounted.current) setSelecting(false); });
  }, [requestedDocument, boundDocument, wantsCreate, selectionNeeded, pendingSoul, saveState, selecting, flush, onPair, onRun, onDocument, onNewAgentHandled]);
  const handledAction = useRef<number | undefined>(undefined);
  const finishAction = () => {
    if (!actionRequest || handledAction.current === actionRequest.id) return;
    handledAction.current = actionRequest.id; onActionHandled?.(actionRequest.id);
  };
  const actionReady = !!actionRequest && actionRequest.id !== handledAction.current && active && (actionRequest.action !== 'run' || !setupPending)
    && handledOpen.current === openDocumentRequest && !selectionNeeded && !selecting && !selectionError && !creating && !wantsCreate && !reloading
    && documentId === boundDocument && actionRequest.documentId === boundDocument && pair?.documentId === boundDocument && saveState === 'saved' && !pendingSoul;
  useEffect(() => {
    if (!actionRequest || actionRequest.id === handledAction.current) return;
    if (!active || actionRequest.action === 'run' && setupPending || actionRequest.documentId !== documentId || creating || wantsCreate || selectionError || saveState === 'error' || error && !pair) { finishAction(); return; }
    if (!actionReady) return;
    setView(actionRequest.action === 'chat' ? 'Chat' : 'Agent');
    if (actionRequest.action !== 'run') finishAction();
  });
  const edit = (value: string, rawFile = false) => {
    const content = rawFile ? value.replace(/\r\n/g, '\n').replaceAll('\n', lineEnding.current) : value;
    if (content === latest.current) return;
    latest.current = content; setDraft(content); setStale(!pair?.source || content !== saved.current); setError(undefined);
    setSaveState('dirty'); clearTimeout(timer.current);
    timer.current = setTimeout(() => { void flush(); }, 500);
  };
  async function reloadSource(prepared = false) {
    if ((reloadAdmitted.current && !prepared) || busy.current || needsCommit.current || pendingSoulCurrent.current || latest.current !== saved.current) return;
    reloadAdmitted.current = true; setReloading(true);
    const request = ++reloadEpoch.current;
    const before = latest.current;
    try {
      const value = await agentCall('agent_open', undefined, boundDocument);
      // A file read must not replace edits made while it was in flight, including
      // the separate soul field whose draft is not yet in latest.current.
      if (request !== reloadEpoch.current || latest.current !== before || saved.current !== before || busy.current || needsCommit.current || pendingSoulCurrent.current) return;
      confirmed.current = value; setPair(value); setDraft(value.agent); latest.current = saved.current = value.agent;
      lineEnding.current = value.agent.includes('\r\n') ? '\r\n' : '\n';
      setStale(false); onPair?.(value, boundDocument);
    } catch (failure) {
      if (request === reloadEpoch.current && !pair) {
        setError('Your agent folder could not be opened. Open it again from Designer.'); onPair?.(undefined, boundDocument);
      }
      throw failure;
    } finally { if (request === reloadEpoch.current) { reloadAdmitted.current = false; setReloading(false); } }
  }
  useEffect(() => {
    if (sampleRevision && boundDocument === SAMPLE_DOCUMENT_ID) void reloadSource(true).catch(() => { /* Keep the current draft on an unreadable refresh. */ });
  }, [sampleRevision, boundDocument]);
  const info = { ...agentInfo(draft), description: readForm(draft).description };
  const nameCounts = new Map<string, number>();
  for (const entry of entries ?? []) nameCounts.set(entry.name, (nameCounts.get(entry.name) ?? 0) + 1);
  const saveLabel = pendingSoul ? 'Unsaved changes' : { saved: '', dirty: 'Unsaved changes', saving: 'Saving…', error: 'Not saved' }[saveState];
  const sendBlocked = !pair ? error ? 'Reopen your agent before sending' : 'Wait for your agent folder to open'
    : reloading ? 'Wait for your saved files to reload' : setupPending ? 'Wait for the local cluster to be ready before sending'
    : saveState !== 'saved' || pendingSoul ? 'Save your changes first' : undefined;
  const chatBlocked = remote ? 'Chat runs on the local cluster. Choose This computer in Clusters.'
    : setupPending ? 'Connect the local cluster and the AI in Clusters before chatting.'
    : documentId !== boundDocument ? 'Wait for your agent to finish saving before switching.'
    : reloading ? 'Wait for your saved files to reload.'
    : pendingSoul ? 'Wait for your personality edit to save. Check Agent view if it needs attention.'
    : saveState !== 'saved' ? saveState === 'error' ? 'Your changes could not be saved. Try saving again in Agent view.' : 'Wait for your changes to save.' : undefined;
  const testSource = pair && <Disclosure className="check-source" label={<>Test file: {pair.name ?? 'web-reader'}.agent.test.md</>}><pre className="example-source" tabIndex={0} aria-label="Test file, read-only">{pair.tests}</pre></Disclosure>;
  return <>
    {creating && <NewAgent active={active} onRegistered={() => { if (onRegistered) onRegistered(); else if (mounted.current) setListAttempt(value => value + 1); }} model={currentCatalog?.reply?.defaultModel ?? undefined} onCreated={async id => {
      setCreating(false); requestDocument(id);
    }} onCancel={() => { setCreating(false); requestAnimationFrame(() => document.querySelector<HTMLButtonElement>('.new-agent-entry')?.focus()); }} />}
    <section className="designer" hidden={creating} aria-labelledby={active && !creating ? 'page-title' : undefined}>
    <div className="page-heading designer-heading">
      <div className="agent-heading-copy"><h1 className="agent-name" id={active && !creating ? 'page-title' : undefined}>{info.name ?? (pair ? 'Agent file' : 'Designer')}</h1>{info.description && <p className="muted">{info.description}</p>}</div>
      <div className="heading-actions">
        <div className="view-switch" role="group" aria-label="Designer view">
          <button type="button" aria-pressed={view === 'Agent'} onClick={() => setView('Agent')}>Agent</button>
          <button type="button" aria-pressed={view === 'Chat'} disabled={!pair} title={!pair ? 'Wait for your agent folder to open' : undefined} onClick={() => setView('Chat')}>Chat</button>
          <button type="button" aria-pressed={view === 'File'} disabled={!pair} title={!pair ? 'Wait for your agent folder to open' : undefined} onClick={() => setView('File')}>File</button>
        </div>
        <label className="agent-selector"><span className="sr-only">Agent</span><select className="field" disabled={!entries || selecting} title={!entries ? 'Reading your agents' : selecting ? 'Opening your agent' : undefined} value={boundDocument} onChange={event => { setSelectionError(undefined); setWantsCreate(false); onNewAgentHandled?.(); requestDocument(event.target.value); }}>
          {!entries?.some(entry => entry.id === boundDocument) && <option value={boundDocument}>{pair?.name ?? 'Current agent'}</option>}
          {entries?.map(entry => <option key={entry.id} value={entry.id}>{entry.name}{(nameCounts.get(entry.name) ?? 0) > 1 ? ` (${entry.path ?? 'location unavailable'})` : ''}{entry.available ? '' : ' (unavailable)'}</option>)}
        </select></label>
        <Button type="button" kind="secondary" className="new-agent-entry" onClick={() => setWantsCreate(true)}>New agent</Button>
        {actions}
        <SendEntry blocked={sendBlocked} onProceed={openSend} />
      </div>
    </div>
    {selectionError && <p role="alert">{selectionError}</p>}
    {!pair ? error ? <div className="designer-file card"><p role="alert">{error}</p><div className="heading-actions"><Button type="button" kind="secondary" onClick={() => { setError(undefined); requestDocument(boundDocument); setOpenAttempt(value => value + 1); }}>Try opening again</Button></div></div> : <div className="designer-file card designer-skeleton" role="status" aria-label="Opening your agent" aria-busy="true"><span className="sr-only">Opening your agent…</span><div aria-hidden="true"><div className="skeleton-field skeleton-prompt" /><div className="skeleton-field" /><div className="skeleton-field" /><div className="skeleton-field" /></div></div> : <>
      <div hidden={view !== 'Chat'}><Chat key={boundDocument} documentId={boundDocument} blocked={chatBlocked} /></div>
      <div hidden={view === 'Chat'} className="designer-work">
      <div className="designer-editing">
      <div className="designer-file card">
        <div className="file-toolbar"><h2>{view === 'File' ? `${pair.name ?? 'web-reader'}.agent.md` : 'Instructions and settings'}</h2><span className="muted" role="status">{saveLabel}</span></div>
        {view === 'File' ? <>
          <FileLocation path={pair.path} documentId={boundDocument} />
          <p className="muted" id="file-hint">The --- block contains your agent’s name, model and tools. Instructions follow it. Your edits save automatically.</p>
          <FileEditor value={draft} onChange={value => edit(value, true)} onSave={() => { void flush(); }} />
        </> : null}
        <div hidden={view !== 'Agent'}>
          <AgentParts source={draft} path={pair.path} documentId={boundDocument} readOnly={false} setupPending={setupPending} catalog={currentCatalog?.reply} catalogLoading={catalogEnabled && !currentCatalog} remote={remote} onPendingEdit={observePendingSoul} onChange={edit} />
          {currentCatalog && (!currentCatalog.reply || currentCatalog.reply.models.state === 'unavailable' || currentCatalog.reply.tools.state === 'unavailable') && <Button type="button" kind="secondary" onClick={() => setCatalogAttempt(attempt => attempt + 1)}>Refresh tools and models</Button>}
        </div>
        {pair.storage && <p className="muted">{storageCopy[pair.storage]}</p>}
        {error && <div className="save-error"><p role="alert">{error}</p><Button kind="primary" className="action-spaced" type="button" onClick={() => { void flush(true); }}>Try saving again</Button></div>}
      </div>
      </div>
      <div className="designer-side">
      <section className="designer-tests" aria-label="This computer"><h2 className="section-label">This computer</h2><div className="card">
        {setupPending ? <div><h3>Tests</h3><span className="coming-control" tabIndex={0} role="group" aria-label="Run tests: Wait for the local cluster and AI to be ready" title="Wait for the local cluster and AI to be ready"><Button type="button" kind="secondary" className="action-spaced" disabled title="Wait for the local cluster and AI to be ready">Run tests</Button><span className="control-tip" aria-hidden="true">Wait for the local cluster and AI to be ready</span></span><p className="muted">The local cluster is getting ready. Connect the AI in Clusters before running a test.</p>{onChooseCluster && <Button type="button" kind="primary" onClick={onChooseCluster}>Go to Clusters</Button>}</div>
          : <Run tests={pair.tests} onChooseCluster={onChooseCluster} onStopLocalCluster={onStopLocalCluster} onEditInstructions={editInstructions} documentId={boundDocument} source={pair.source} onSnapshot={observeRun} blocked={pendingSoul || saveState !== 'saved'} stale={stale} startRequest={actionReady && view === 'Agent' && actionRequest?.action === 'run' ? actionRequest.id : undefined} onStartRequestHandled={finishAction} onStart={() => { if (latest.current === draft) setStale(false); }} />}
        {/* Run shows the test list and test file itself; this copy is only for the not-ready state. */}
        {setupPending && <><DesignerTests definitions={testDefinitions(pair.tests)} stale={false} /><Details hint="Test file">{testSource}</Details></>}
      </div></section>
      <section className="designer-checks" aria-label="Team cluster">
        <Send expanded={sendRequest > 0} documentId={boundDocument} place={place} source={pair.source} model={readForm(draft).model} local={localRun} stale={stale} blocked={sendBlocked} onSeeInOverview={onSeeInOverview} onFixLocal={focusLocalResult} onChooseCluster={onChooseCluster} onReloadSource={reloadSource} />
      </section>
      </div>
      </div>
    </>}
    {!pair && (remote || error && isTauri()) && <div className="designer-checks"><SendHistory revision={boundDocument} /></div>}
  </section></>;
}
