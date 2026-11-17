import { Disclosure } from '../components/Disclosure';
import { Button } from '../components/Button';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { SAMPLE_DOCUMENT_ID, type SourceId, sameSource } from '../designer/agent-api';
import type { RunSnapshot } from '../designer/run-api';
import { modelLabel } from '../designer/model-label';
import type { SelectedPlace } from './api';
import { type AttemptSummary, type Cancellation, type SendCheck, type SendHistoryDetail, type SendStatus, sendCall, sendErrors, sendHistory, sendHistoryDetail, sendPreflight } from './send-api';
import './clusters.css';
import { CardSection, Details, Elapsed, GeneratedValue, LongCard, type Status, StatusIcon, StatusPill } from '../components/DesignSystem';
import { changedFiles, resultAnnouncement, TestProgress, TestResults } from '../components/TestResults';

/** Opens the Send card; only the card's submit action owns the failed-test warning. */
export function SendEntry({ blocked, onProceed }: { blocked?: string; onProceed: () => void }) {
  return <div className="send-entry">
    <Button type="button" kind="secondary" disabled={!!blocked} title={blocked} onClick={onProceed}>Send to a cluster</Button>
  </div>;
}
const edge = 8;
/** Non-modal platform popover placed with measured coordinates, so it overlays rather than moves the page. */
function SendWarning({ anchor, test, onFix, onSend, onDismiss }: { anchor: HTMLElement; test: string; onFix: () => void; onSend: () => void; onDismiss: (focusAnchor: boolean) => void }) {
  const popover = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState<{ top: number; left: number; width: number }>();
  useLayoutEffect(() => {
    const element = popover.current;
    element?.showPopover?.();
    const position = () => {
      const rect = anchor.getBoundingClientRect();
      const width = Math.min(340, window.innerWidth - 2 * edge); const height = element?.offsetHeight ?? 0;
      const below = rect.bottom + edge;
      setBox({ width, left: Math.max(edge, Math.min(rect.left, window.innerWidth - width - edge)), top: Math.max(edge, below + height > window.innerHeight - edge ? rect.top - height - edge : below) });
    };
    position(); window.addEventListener('resize', position);
    element?.querySelector<HTMLElement>('[data-initial-focus]')?.focus();
    return () => window.removeEventListener('resize', position);
  }, [anchor]);
  return <div role="dialog" aria-label="Local test failed" ref={popover} popover="auto" className="send-warning" style={box}
    onToggle={event => {
      // Light dismissal: hand focus back unless the user already moved it elsewhere.
      if (event.newState === 'closed') onDismiss(popover.current?.contains(document.activeElement) === true || document.activeElement === document.body);
    }}
    onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); onDismiss(true); } }}>
    <p>Local test ‘{test}’ failed. You can still send this agent.</p>
    <div className="workspace-step-actions">
      <Button type="button" kind="primary" data-initial-focus onClick={onFix}>Fix it first</Button>
      <Button type="button" kind="secondary" onClick={onSend}>Send anyway</Button>
    </div>
  </div>;
}
const words = { passed: 'pass', failed: 'fail', unknown: 'unknown', cancelled: 'cancelled' } as const;
const cancellationLabels = { cancelling: 'Cancelling tests', requested: 'Cancellation requested', confirmed: 'Tests cancelled', unconfirmed: 'Cancellation not confirmed', denied: 'Cannot cancel', unsupported: 'Cannot cancel' };
function CancellationNotice({ cancellation, report }: { cancellation?: Cancellation | null; report: SendHistoryDetail['report'] }) {
  if (!cancellation || cancellation === 'cancelling') return null;
  const messages = {
    requested: 'Cancellation requested. Waiting for the cluster.',
    confirmed: report?.cases.some(c => c.verdict === 'cancelled') ? 'Tests cancelled on the cluster. Completed answers kept.' : report && report.verdict !== 'unknown' && report.cases.length > 0 ? 'Tests finished before cancellation.' : 'Test results could not be confirmed. Check cluster status.',
    unconfirmed: 'Cancellation not confirmed. Work may still be running.',
    denied: 'Cannot cancel with this cluster identity',
    unsupported: 'This Orka cannot cancel tests. Work may still be running.',
  };
  return <><p role="status">{messages[cancellation]}</p>
    {cancellation === 'denied' && <p>Work may still be running.</p>}</>;
}
const cancellationDetails = 'Orka marked these tests cancelled. Worker cleanup can continue; actions already taken are not undone.';
function SavedSend({ attempt, revision, onChanged }: { attempt: AttemptSummary; revision: string; onChanged?: () => void }) {
  const [open, setOpen] = useState(false);
  const [detail, setDetail] = useState<SendHistoryDetail>();
  const [checkedVerdict, setCheckedVerdict] = useState<AttemptSummary['verdict']>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const removeInvoker = useRef<HTMLButtonElement>(null);
  const keepDetails = useRef<HTMLButtonElement>(null);
  const savedDisclosure = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!confirmRemove) return;
    keepDetails.current?.focus();
    return () => { (removeInvoker.current ?? savedDisclosure.current)?.focus(); };
  }, [confirmRemove]);
  const operation = useRef(false);
  const epoch = useRef(0);
  useEffect(() => () => { epoch.current++; }, []);
  useEffect(() => {
    if (!open) { setDetail(undefined); return; }
    if (operation.current) return;
    let mounted = true;
    const request = ++epoch.current;
    void sendHistoryDetail('send_history_details', attempt.id).then(value => {
      if (mounted && request === epoch.current) { setDetail(value); setError(undefined); }
    }, (failure: Error) => { if (mounted && request === epoch.current) setError(failure.message); });
    return () => { mounted = false; };
  }, [open, attempt.id, revision]);
  async function act(command: 'send_history_check' | 'send_history_remove' | 'send_history_cancel') {
    if (operation.current) return;
    operation.current = true; setBusy(true); setCancelling(command === 'send_history_cancel');
    const request = ++epoch.current;
    try {
      const value = await sendHistoryDetail(command, attempt.id);
      if (request === epoch.current) {
        setDetail(value); setError(undefined); setConfirmRemove(false);
        if (command !== 'send_history_remove') setCheckedVerdict(value.interrupted ? 'unknown' : value.report?.verdict ?? 'unknown');
        onChanged?.();
      }
    } catch (failure) { if (request === epoch.current) setError((failure as Error).message); }
    finally { operation.current = false; if (request === epoch.current) { setBusy(false); setCancelling(false); } }
  }
  const verdict = detail?.interrupted ? 'unknown' : detail?.report?.verdict ?? checkedVerdict ?? attempt.verdict;
  return <li>
    <div className="send-history-heading"><strong>{attempt.targetDisplay}</strong><span>Saved tests: {verdict === 'running' ? 'unknown' : words[verdict]}</span></div>
    <p>{attempt.plannedAgentName}</p><p>AI connection: {attempt.provider} · <time dateTime={new Date(attempt.time * 1000).toISOString()}>{new Date(attempt.time * 1000).toLocaleString()}</time></p>
    <Disclosure label="Show saved details" summaryRef={savedDisclosure} onToggle={event => { setOpen(event.currentTarget.open); if (!event.currentTarget.open) setConfirmRemove(false); }}>
      {open && <>
        {error && <p role="alert">{error}</p>}
        {!detail && !error && <p role="status">Reading saved details…</p>}
        {detail && <>
          {detail.interrupted && !detail.cancellation && <p role="status">Last seen running; current result unknown</p>}
          <CancellationNotice cancellation={cancelling ? 'cancelling' : detail.cancellation} report={detail.report} />
          {cancelling && <p role="status">Cancelling tests…</p>}
          {detail.detailState === 'legacy' && <p>This earlier Send has a summary only.</p>}
          {detail.detailState === 'removed' && <p>Saved details removed from this computer.</p>}
          {detail.detailState === 'omitted' && <p>Some details could not be saved.</p>}
          {detail.report && <><p>Saved on this computer</p>{detail.capturedAt !== null && <p>Captured <time dateTime={new Date(detail.capturedAt * 1000).toISOString()}>{new Date(detail.capturedAt * 1000).toLocaleString()}</time></p>}
            <TestResults report={detail.report} stale={false} /></>}
          {detail.availability === 'missing' && <p role="status">No longer available from the cluster</p>}
          {detail.availability === 'unavailable' && <p role="status">Cluster status unavailable. Reconnect this cluster in Clusters and try again. Saved answers are unchanged.</p>}
          {detail.availability === 'available' && <p role="status">Cluster status checked. No tests were submitted.</p>}
          {confirmRemove && <p role="alert">Remove saved details from this computer? Cluster data stays unchanged.</p>}
          <div className="workspace-step-actions">
            {confirmRemove ? <>
              <Button type="button" kind="secondary" className="send-history-remove" disabled={busy} onClick={() => { void act('send_history_remove'); }}>Remove local copy</Button>
              <Button ref={keepDetails} type="button" kind="secondary" disabled={busy} onClick={() => setConfirmRemove(false)}>Keep saved details</Button>
            </> : <>
              {detail.canCheck && <Button type="button" kind="secondary" disabled={busy} onClick={() => { void act('send_history_check'); }}>Check cluster status</Button>}
              {detail.canCancel && detail.cancellation !== 'unsupported' && <Button type="button" kind="secondary" disabled={busy} onClick={() => { void act('send_history_cancel'); }}>{detail.cancellation ? 'Retry cancellation' : 'Cancel tests'}</Button>}
              {detail.canRemove && <Button ref={removeInvoker} type="button" kind="quiet" className="send-history-remove" disabled={busy} onClick={() => setConfirmRemove(true)}>Remove saved details</Button>}
            </>}
          </div>
          {busy && <p role="status">Updating saved details…</p>}
        </>}
      </>}
      <Details hint="Saved identity"><dl className="cluster-details">
        <div><dt>Agent UID</dt><dd>{attempt.agentUid ?? 'Not confirmed'}</dd></div>
        <div><dt>Agent file hash</dt><dd>{attempt.source.agent}</dd></div>
        <div><dt>Test file hash</dt><dd>{attempt.source.tests}</dd></div>
        {detail?.model && <div><dt>Model</dt><dd>{detail.model}</dd></div>}
        {detail?.report?.cases.some(c => c.verdict === 'cancelled') && <p>{cancellationDetails}</p>}
      </dl></Details>
    </Disclosure>
  </li>;
}
export function SendHistory({ revision, onChanged }: { revision: string; onChanged?: () => void }) {
  const [history, setHistory] = useState<AttemptSummary[]>();
  const [error, setError] = useState<string>();
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let mounted = true;
    void sendHistory().then(value => {
      if (mounted) { setHistory(value); setError(undefined); }
    }, (failure: Error) => { if (mounted) setError(failure.message); });
    return () => { mounted = false; };
  }, [revision, retry]);
  return <section className="send-history" aria-label="Past Sends"><Disclosure label="Past Sends">
    <p>Earlier Sends and their test results. An interrupted Send may still be running on the cluster.</p>
    <Details hint="Saved results and limits"><p>Up to 256 Sends, 64 MiB total and 4 MiB per Send. Old terminal details are evicted first. Interrupted Sends are not resumed. Checking their status never sends tests again. Removing saved details changes only this computer, not the cluster. Removed or evicted answers may not be recoverable.</p></Details>
    {error ? <><p role="alert">{error}</p><Button type="button" kind="secondary" onClick={() => setRetry(v => v + 1)}>Retry saved Sends</Button></>
      : history ? history.length ? <ol>{history.map(attempt => <SavedSend key={attempt.id} attempt={attempt} revision={revision} onChanged={onChanged} />)}</ol> : <p>No saved Sends yet.</p> : <p role="status">Reading saved Sends…</p>}
  </Disclosure></section>;
}
export default function Send({ expanded = true, documentId = SAMPLE_DOCUMENT_ID, place, source, model, local, stale, blocked, onChooseCluster, onReloadSource, onSeeInOverview, onFixLocal }: {
  expanded?: boolean; documentId?: string; place: SelectedPlace; source?: SourceId; model?: string; local?: RunSnapshot; stale: boolean;
  blocked?: string; onChooseCluster?: () => void; onReloadSource?: () => Promise<void>;
  /** Opens Overview on the agent recorded for this native Send attempt. */
  onSeeInOverview?: (sendId: string) => void; onFixLocal?: (testId: string) => void;
}) {
  const [status, setSnapshot] = useState<SendStatus>();
  const snapshot = status?.documentId === documentId ? status.view : null;
  const foreignBusy = status?.documentId === documentId && status.busy?.documentId !== documentId ? status.busy : null;
  const [message, setMessage] = useState<string>();
  const [controlCancelling, setControlCancelling] = useState(false);
  const [busy, setBusy] = useState(false);
  const [historyRevision, setHistoryRevision] = useState(0);
  const admitted = useRef(false); const epoch = useRef(0);
  useEffect(() => {
    let mounted = true; let polling = false;
    admitted.current = false; setBusy(false); setControlCancelling(false); setMessage(undefined); setReloadError(undefined);
    async function observe() {
      if (polling || admitted.current) return;
      polling = true; const request = epoch.current;
      try { const result = await sendCall('send_status', { documentId }); if (mounted && request === epoch.current) { setSnapshot(result); setMessage(undefined); } }
      catch (error) { if (mounted && request === epoch.current) setMessage((error as Error).message); }
      finally { polling = false; }
    }
    void observe(); const timer = window.setInterval(() => { void observe(); }, 500);
    return () => { mounted = false; epoch.current++; window.clearInterval(timer); };
  }, [documentId]);
  const target = place.kind === 'cluster' ? place.target : undefined;
  const providerId = place.kind === 'cluster' ? place.providerId : undefined;
  const provider = place.kind === 'cluster' ? place.providers?.find(p => p.id === providerId) : undefined;
  const [refresh, setRefresh] = useState(0);
  const [check, setCheck] = useState<{ key: string; value?: SendCheck; error?: string }>();
  const [reloadError, setReloadError] = useState<string>();
  const targetId = target?.id;
  const agentHash = source?.agent; const testsHash = source?.tests;
  const previousSend = useRef<{ id: string; state: string } | undefined>(undefined);
  const previousBusy = useRef<string | undefined>(undefined);
  const busyDocumentId = foreignBusy?.documentId;
  const sendId = snapshot?.id; const sendState = snapshot?.state;
  useEffect(() => {
    const previous = previousSend.current;
    previousSend.current = sendId && sendState ? { id: sendId, state: sendState } : undefined;
    const foreignFinished = !!previousBusy.current && !busyDocumentId;
    previousBusy.current = busyDocumentId;
    if (foreignFinished || (previous?.id === sendId && previous?.state === 'running' && sendState !== 'running')) setRefresh(value => value + 1);
  }, [sendId, sendState, busyDocumentId]);
  const checkKey = JSON.stringify([documentId, targetId, providerId, agentHash, testsHash, refresh]);
  const checkingEnabled = !blocked && !!targetId && !!providerId && !!agentHash && !!testsHash;
  useEffect(() => {
    if (!checkingEnabled || !targetId || !providerId) return;
    let mounted = true;
    void sendPreflight(targetId, providerId, documentId).then(value => {
      if (mounted) setCheck({ key: checkKey, value });
    }, (error: Error) => { if (mounted) setCheck({ key: checkKey, error: error.message }); });
    return () => { mounted = false; };
  }, [checkingEnabled, checkKey, targetId, providerId, documentId]);
  const checked = checkingEnabled && check?.key === checkKey ? check : undefined;
  const matched = checked?.value && sameSource(source, checked.value.source) === true;
  const mismatched = checked?.value?.source != null && !matched;
  const reason = blocked ?? (foreignBusy ? `${foreignBusy.name} is sending tests.` : undefined) ?? (!target || !provider ? 'Choose a cluster and AI connection in Clusters first'
    : !agentHash || !testsHash ? 'Saved file versions are unavailable. Open your agent again before sending.'
    : !checked ? 'Checking this destination…' : checked.error ?? (mismatched ? 'Your saved files changed. Reload them to check this destination.'
    : !checked.value?.canSend ? 'Resolve the destination checks before sending.' : undefined));
  const canSeparate = !blocked && !foreignBusy && matched && !!checked?.value?.canSeparate && !!target && !!provider;
  // Only the first Send creates; every later one is an update of the linked agent. Rerunning an
  // unchanged agent is a result action (below), never the card's primary wording.
  const action = checked?.value?.action;
  const sendLabel = !action || action === 'create' ? 'Send and run tests' : 'Update agent and run tests';
  const againLabel = action === 'unchanged' ? 'Run cluster tests again' : sendLabel;
  async function mutate(command: 'send_start' | 'send_cancel', separateAgent = false) {
    if (admitted.current) return;
    let args: Record<string, unknown>;
    if (command === 'send_start') {
      if ((separateAgent ? !canSeparate : !!reason) || !target || !providerId || !checked?.value?.source) return;
      args = { targetId: target.id, providerId, expectedSource: checked.value.source, ...(separateAgent ? { separateAgent: true } : {}) };
    } else {
      if (!snapshot) return;
      args = { sendId: snapshot.id };
    }
    admitted.current = true; setBusy(true); const request = ++epoch.current;
    try {
      const result = await sendCall(command, { ...args, documentId });
      if (request === epoch.current) { setSnapshot(result); setMessage(undefined); }
    } catch (error) { if (request === epoch.current) setMessage((error as Error).message); }
    finally { if (request === epoch.current) { admitted.current = false; setBusy(false); } }
  }
  async function control(command: 'send_history_check' | 'send_history_cancel') {
    if (admitted.current || !snapshot) return;
    admitted.current = true; setBusy(true); const request = ++epoch.current;
    setControlCancelling(command === 'send_history_cancel');
    try {
      await sendHistoryDetail(command, snapshot.id);
      setHistoryRevision(value => value + 1);
      setRefresh(value => value + 1);
      const result = await sendCall('send_status', { documentId });
      if (request === epoch.current) { setSnapshot(result); setMessage(undefined); }
    } catch (error) { if (request === epoch.current) setMessage((error as Error).message); }
    finally { if (request === epoch.current) { admitted.current = false; setBusy(false); setControlCancelling(false); } }
  }
  const running = snapshot?.state === 'running';
  const cancellation = snapshot?.cancellation;
  const cancelling = controlCancelling || cancellation === 'cancelling';
  const retryCancel = !running && (cancellation === 'requested' || cancellation === 'unconfirmed' || cancellation === 'denied');
  const checkCancellation = !running && !!cancellation && (cancellation !== 'confirmed' || !snapshot?.report || snapshot.report.verdict === 'unknown');
  const samePlace = snapshot?.targetId === target?.id && snapshot?.providerId === providerId;
  // A temporary reload block must not promote cached editor hashes to fresh evidence.
  const resultSource = target && providerId ? checked?.value?.source ?? undefined : source;
  const editorCurrent = sameSource(source, snapshot?.source) === true;
  const current = !stale && editorCurrent && sameSource(resultSource, snapshot?.source) === true;
  const localCurrent = !stale && local?.sourceCurrent !== false && sameSource(source, local?.source) === true && sameSource(resultSource, local?.source) === true;
  const unconfirmed = !!snapshot?.source && !stale && editorCurrent && !resultSource;
  const targetName = snapshot?.targetName ?? (snapshot?.targetId === target?.id ? target?.context_name : undefined);
  const outdated = !!snapshot && (!samePlace || (!!snapshot.source && !current));
  const revisionExplanation = snapshot?.source && !current ? changedFiles(editorCurrent ? resultSource : source, snapshot.source, stale) : undefined;
  const uncertain = snapshot?.state === 'unknown' || snapshot?.report?.verdict === 'unknown';
  const unhealthy = !!cancellation && cancellation !== 'confirmed';
  const needsAttention = !!snapshot?.error && (snapshot.error.startsWith('update') && snapshot.error !== 'updateUnconfirmed' || snapshot.error === 'earlierWork');
  // One pill word per piece of evidence; a ready destination is never presented as a pass.
  const [resultState, pillWord]: [Status, string | undefined] = running || busy ? ['running', cancelling ? 'Cancelling tests' : 'Running tests']
    : outdated ? ['stale', !samePlace ? 'Earlier result' : unconfirmed ? 'Revision not confirmed' : 'Earlier files']
    : unhealthy ? ['needs-you', cancellationLabels[cancellation]]
    : needsAttention ? ['needs-you', 'Needs attention']
    : uncertain ? ['stale', 'Result unknown']
    : snapshot?.report?.verdict === 'passed' ? ['passed', 'Passed']
    : snapshot?.report?.verdict === 'failed' ? ['failed', 'Failed']
    : snapshot?.report?.verdict === 'cancelled' ? ['not-started', 'Cancelled']
    : snapshot?.error ? needsAttention ? ['needs-you', 'Needs attention'] : ['failed', 'Failed']
    : checkingEnabled && !checked ? ['running', 'Checking destination']
    : !target || !provider || !agentHash || !testsHash || blocked ? ['not-started', 'Not started']
    : reason ? ['needs-you', 'Needs attention'] : ['passed', 'Ready to send'];
  const passedCount = snapshot?.report?.cases.filter(c => c.verdict === 'passed').length ?? 0;
  const headline = outdated ? 'Earlier result' : running || busy ? 'Tests on your cluster' : unhealthy ? (cancellation === 'requested' ? 'Waiting for the cluster' : 'Tests may still be running')
    : needsAttention ? 'Send needs attention' : uncertain ? 'Outcome not confirmed' : snapshot?.report?.verdict === 'cancelled' ? 'Tests cancelled on the cluster'
    : snapshot?.report ? `${passedCount} of ${snapshot.report.cases.length} tests passed` : 'Send your agent';
  // A known failure on exactly the current saved files (the same comparison the "This computer" row uses).
  const failing = localCurrent ? local?.report?.cases.find(c => c.verdict === 'failed') : undefined;
  const panel = useRef<HTMLElement>(null);
  const [warning, setWarning] = useState<{ anchor: HTMLElement; separate: boolean }>();
  const showWarning = !!warning && !!failing && (warning.separate ? canSeparate : !reason) && !busy && !running;
  useEffect(() => {
    if (!warning || showWarning) return;
    // Saving, a source change or a blocked destination invalidates the warning: no enabled Send anyway.
    setWarning(undefined);
    if (warning.anchor.isConnected && !(warning.anchor as HTMLButtonElement).disabled) warning.anchor.focus(); else panel.current?.focus();
  }, [warning, showWarning]);
  const startSend = (anchor: HTMLElement, separate = false) => { if (failing) setWarning({ anchor, separate }); else void mutate('send_start', separate); };
  const seeOverview = !running && !!onSeeInOverview && !!snapshot
    // A reused agent is already owned even when an update's Stop left this view without its receipt; native re-checks the recorded UID.
    && (snapshot.acknowledged.some(r => r.kind === 'Agent' && r.uid) || snapshot.agentAction === 'update' || snapshot.agentAction === 'unchanged');
  const stages: Record<string, string> = { ...sendErrors, capture: 'Reading your saved files', access: 'Confirming cluster access', preflight: 'Checking the AI connection and tools', storage: 'Saving the Send summary', submit: 'Starting your tests', observe: 'Waiting for answers and evaluating tests', stopped: 'Stopped watching', done: 'Tests finished' };
  const localEvidence = !local?.report ? 'Not run' : localCurrent ? ({ passed: 'Passed on these files', failed: 'Failed', unknown: 'Result unknown', cancelled: 'Cancelled' })[local.report.verdict]
    : local.sourceCurrent === false || sameSource(source, local.source) === false ? 'Earlier files' : 'Revision not confirmed';
  const checkRows = checked?.value && !mismatched && <ul>{checked.value.rows.map(row => <li key={`${row.label}-${row.message}`} data-state={row.status}>
    <span className="preflight-state"><StatusIcon state={row.status === 'allowed' ? 'passed' : row.status === 'blocked' ? 'needs-you' : row.status === 'cannotCheck' ? 'needs-you' : 'not-started'} />{{ allowed: 'Ready', blocked: 'Blocked', cannotCheck: 'Can’t check', unverified: 'Unverified' }[row.status]}</span>
    <div><b>{row.label}</b><p>{row.message}</p></div>
  </li>)}</ul>;
  const refreshDestination = (kind: 'primary' | 'secondary') => mismatched ? <Button type="button" kind={kind} disabled={busy || running || !onReloadSource} onClick={() => {
    setReloadError(undefined);
    void onReloadSource?.().catch(() => setReloadError('Your saved files could not be reloaded. Your draft is still here.'));
  }}>Reload saved files</Button> : <Button type="button" kind={kind} disabled={busy || running} onClick={() => setRefresh(v => v + 1)}>Check again</Button>;
  // Recovery takes precedence over navigation, even when an agent was created successfully.
  // Status/cancellation act on the recorded Send; a new Send still uses the selected destination.
  const recovery = checkCancellation ? 'status' : mismatched ? 'reload' : !target || !provider ? 'choose' : checked && reason ? 'check' : undefined;
  const passedResult = !outdated && snapshot?.report?.verdict === 'passed';
  const sendButton = (kind: 'primary' | 'secondary') => <Button type="button" kind={kind} disabled={!!reason || busy || !!message} title={reason} onClick={event => startSend(event.currentTarget)}>{snapshot ? againLabel : sendLabel}</Button>;
  const overviewButton = (kind: 'primary' | 'secondary') => seeOverview && snapshot && onSeeInOverview && <Button type="button" kind={kind} onClick={() => onSeeInOverview(snapshot.id)}>See in Overview</Button>;
  const retryCancellation = retryCancel && <Button type="button" kind="secondary" disabled={busy} onClick={() => { void control('send_history_cancel'); }}>Retry cancellation</Button>;
  const primaryAction = recovery === 'status' ? <Button type="button" kind="primary" disabled={busy} onClick={() => { void control('send_history_check'); }}>Check cluster status</Button>
    : recovery === 'reload' || recovery === 'check' ? refreshDestination('primary')
    : recovery === 'choose' ? <Button type="button" kind="primary" onClick={onChooseCluster} disabled={!onChooseCluster}>Choose a cluster</Button>
    : passedResult && seeOverview ? overviewButton('primary') : sendButton('primary');
  const secondaryAction = recovery === 'status' ? retryCancellation : recovery ? overviewButton('secondary')
    : passedResult && seeOverview ? sendButton('secondary') : overviewButton('secondary');
  // canSend means an attempt is permitted; advisory "unverified" rows (inference, create proof) stay with the collapsed checks.
  const destinationReady = !!checked?.value?.canSend && !mismatched;
  const destinationModel = checked?.value?.model ?? model;
  const hasHappened = !!(foreignBusy || message || snapshot || controlCancelling);
  // Only an unused local Send may be hidden. Keep all existing work and recovery visible.
  const visible = expanded || !!target || !!snapshot || !!foreignBusy || busy || !!message;
  return <>{visible && <h2 className="section-label">Team cluster</h2>}<section hidden={!visible} ref={panel} tabIndex={-1} className="send-panel card" aria-label="Send and cluster tests">
    <LongCard title={headline} status={<><StatusPill state={resultState}>{pillWord}</StatusPill><span className="status-description">{outdated ? unconfirmed ? undefined : 'Not run since your edit' : running || busy || !snapshot ? undefined : unhealthy ? undefined : uncertain ? snapshot.stage === 'stopped' && !cancellation ? 'Stopped watching' : undefined : undefined}</span></>} summary={<>The saved agent and test files go together{snapshot?.model && <> · <code>{modelLabel(snapshot.model)}</code></>}{targetName && <> · {samePlace ? 'Result on ' : 'Earlier result on '}<code>{targetName}</code></>}.</>}
      action={<div className="workspace-step-actions">
        {running ? <Button type="button" kind="secondary" disabled={busy || cancelling} onClick={() => { void mutate('send_cancel'); }}>Cancel tests</Button>
          : <>{primaryAction}{secondaryAction}</>}
      </div>}>
    {showWarning && failing && <SendWarning anchor={warning.anchor} test={failing.id}
      onFix={() => { setWarning(undefined); onFixLocal?.(failing.id); }}
      onSend={() => { setWarning(undefined); void mutate('send_start', warning.separate); }}
      onDismiss={focusAnchor => { setWarning(undefined); if (focusAnchor) warning.anchor.focus(); }} />}
   <p className="sr-only test-announcement" role="status" aria-live="polite" aria-atomic="true">{!running ? outdated ? revisionExplanation : cancellation && cancellation !== 'confirmed' ? `${cancellationLabels[cancellation]}.` : snapshot?.stage === 'stopped' && !cancellation ? 'Stopped watching.' : uncertain ? 'Test result unknown.' : resultAnnouncement(snapshot?.report) : undefined}</p>
    {!checkingEnabled && reason && (blocked || !foreignBusy) && <p className="muted">{reason}</p>}
    <CardSection title="What you send">
    <dl className="cluster-details"><div><dt>Source</dt><dd>Your saved agent and test files</dd></div>
      <div><dt>This computer</dt><dd>{localEvidence}</dd></div>
      {target && <div><dt>Destination</dt><dd><code>{target.context_name}</code> · <code>{target.watched_namespace}</code></dd></div>}
      {provider && <div><dt>AI connection</dt><dd>{destinationModel ? modelLabel(destinationModel) : provider.name}{provider.ready ? ', checked when tests run' : ' · AI connection not ready'}</dd></div>}
    </dl>
    {checkingEnabled && !destinationReady && <section className="send-preflight" aria-label="Before Send" aria-busy={!checked}>
     <h4>Before Send</h4>
      {checkRows}
      {reason && <p role={checked ? 'alert' : 'status'}>{reason}</p>}
      {!checked && <Elapsed />}
     {reloadError && <p role="alert">{reloadError}</p>}
    </section>}
    </CardSection>
    {hasHappened && <CardSection title="What happened">
    {foreignBusy && <p role="status">{foreignBusy.name} is sending tests.</p>}
    {message && <p role="alert">{message}</p>}
    {snapshot?.storageError && snapshot.error !== 'storage' && <p role="alert">Bridge could not save this Send. Its saved summary may be missing or unknown.</p>}
    {(running || controlCancelling) && <TestProgress stage={cancelling ? 'Cancelling tests…' : stages[snapshot!.stage]} />}
    {snapshot?.agentAction && snapshot.agentAction !== 'unchanged' && snapshot.acknowledged.some(resource => resource.kind === 'Agent') && <p role="status">{{ create: 'Agent created on the cluster.', update: 'Agent updated on the cluster.' }[snapshot.agentAction]}</p>}
    <CancellationNotice cancellation={cancelling ? 'cancelling' : cancellation} report={snapshot?.report ?? null} />
    {snapshot?.stage === 'stopped' && !cancellation && <p role="status">Stopped watching. The cluster may still be running checks.</p>}
    {snapshot?.error && snapshot.stage !== 'stopped' && !cancellation && <p role="alert">{sendErrors[snapshot.error as keyof typeof sendErrors]}</p>}
    {snapshot && !samePlace && <p role="status">This result belongs to another cluster or AI connection, not your current selection.</p>}
    {revisionExplanation && <p className="muted">{revisionExplanation}</p>}
    {outdated && uncertain && !snapshot?.report && <p>Earlier result unknown</p>}
    <TestResults report={snapshot?.report} stale={outdated} />
    </CardSection>}
    {checkingEnabled && destinationReady && <Disclosure className="send-checks" label="Destination ready" hint="Show checks" aria-busy={!checked}>
      {checkRows}{reloadError && <p role="alert">{reloadError}</p>}
    </Disclosure>}
    <Details hint="Resources and file hashes">
      {!running && <div className="workspace-step-actions">
        {recovery && sendButton('secondary')}
        {recovery === 'status' && overviewButton('secondary')}
        {recovery !== 'choose' && (!target || !provider) && <Button type="button" kind="secondary" onClick={onChooseCluster} disabled={!onChooseCluster}>Choose a cluster</Button>}
        {canSeparate && <Button type="button" kind="secondary" disabled={busy} onClick={event => startSend(event.currentTarget, true)}>Send as a separate agent</Button>}
        {checkingEnabled && checked && recovery !== 'reload' && recovery !== 'check' && refreshDestination('secondary')}
      </div>}
      {!running && canSeparate && <p>Sends as a new agent; the old one keeps running.</p>}
      {provider && <p>AI connection: <code>{provider.name}</code></p>}
      {snapshot && <><dl className="cluster-details"><div><dt>Result destination</dt><dd>{targetName ?? 'Not confirmed'}</dd></div>
      <div><dt>Model sent</dt><dd><code>{snapshot.model ?? 'Not captured'}</code></dd></div>
    </dl>
      {snapshot.agentName && <GeneratedValue label="Planned agent" value={snapshot.agentName} />}
      {snapshot.source && <><GeneratedValue label="Agent file hash" value={snapshot.source.agent} /><GeneratedValue label="Test file hash" value={snapshot.source.tests} /></>}
      {snapshot.acknowledged.map(r => <GeneratedValue key={`${r.kind}-${r.uid}`} label={r.kind} value={r.name} />)}
      {snapshot.pending && <GeneratedValue label={`Possibly created ${snapshot.pending.kind}`} value={snapshot.pending.name} />}</>}
      <p>Repeated Sends use the same linked agent, kept on the cluster even if a test fails. Sending requires permission to create an agent and run its tests. Updating a changed agent also requires permission to patch that agent; unchanged tests do not. Saved answers and test receipts remain in Past Sends on this computer, within its retention limits. Cancel tests requests cancellation of this Send's unfinished tests. It keeps the agent, tests and completed answers. Closing Bridge only ends watching.
        {snapshot?.report?.cases.some(c => c.verdict === 'cancelled') && <> {cancellationDetails}</>}</p>
    </Details>
    </LongCard>
  </section><SendHistory onChanged={() => setRefresh(value => value + 1)} revision={`${historyRevision}:${documentId}:${status?.busy?.documentId ?? ''}:${snapshot?.id ?? ''}:${snapshot?.state ?? ''}:${snapshot?.stage ?? ''}:${snapshot?.cancellation ?? ''}:${snapshot?.storageError ?? false}`} /></>;
}
