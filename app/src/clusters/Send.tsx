import { useEffect, useRef, useState } from 'react';
import { SAMPLE_DOCUMENT_ID, type SourceId, sameSource } from '../designer/agent-api';
import type { RunSnapshot } from '../designer/run-api';
import type { SelectedPlace } from './api';
import { type AttemptSummary, type SendHistoryDetail, type SendCheck, type SendStatus, sendCall, sendErrors, sendHistory, sendHistoryDetail, sendPreflight } from './send-api';
import './clusters.css';
import { CardSection, Details, Elapsed, GeneratedValue, LongCard, type Status, StatusIcon, StatusPill } from '../components/DesignSystem';
import { changedFiles, resultAnnouncement, resultLabel, TestProgress, TestResults } from '../components/TestResults';

export function SendEntry({ failingCheck, blocked, onProceed, onFix }: { failingCheck?: string; blocked?: string; onProceed: () => void; onFix: () => void }) {
  const [warning, setWarning] = useState(false);
  return <div className="send-entry">
    <button type="button" className="shell-button" disabled={!!blocked} title={blocked} onClick={() => { if (failingCheck) setWarning(true); else onProceed(); }}>Send to a cluster</button>
    {warning && failingCheck && <div className="send-warning" role="note"><p>Test {failingCheck} failed. You can still send this agent.</p>
      <button type="button" className="workspace-secondary" disabled={!!blocked} onClick={() => { setWarning(false); onProceed(); }}>Send anyway</button>
      <button type="button" className="workspace-disclosure" onClick={() => { setWarning(false); onFix(); }}>Fix it first</button>
    </div>}
  </div>;
}
const words = { passed: 'pass', failed: 'fail', unknown: 'unknown' } as const;
function SavedSend({ attempt, revision }: { attempt: AttemptSummary; revision: string }) {
  const [open, setOpen] = useState(false);
  const [detail, setDetail] = useState<SendHistoryDetail>();
  const [checkedVerdict, setCheckedVerdict] = useState<AttemptSummary['verdict']>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
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
  async function act(command: 'send_history_check' | 'send_history_remove') {
    if (operation.current) return;
    operation.current = true; setBusy(true);
    const request = ++epoch.current;
    try {
      const value = await sendHistoryDetail(command, attempt.id);
      if (request === epoch.current) {
        setDetail(value); setError(undefined); setConfirmRemove(false);
        if (command === 'send_history_check') setCheckedVerdict(value.interrupted ? 'unknown' : value.report?.verdict ?? 'unknown');
      }
    } catch (failure) { if (request === epoch.current) setError((failure as Error).message); }
    finally { operation.current = false; if (request === epoch.current) setBusy(false); }
  }
  const verdict = detail?.interrupted ? 'unknown' : detail?.report?.verdict ?? checkedVerdict ?? attempt.verdict;
  return <li>
    <div className="send-history-heading"><strong>{attempt.targetDisplay}</strong><span>Saved tests: {verdict === 'running' ? 'unknown' : words[verdict]}</span></div>
    <p>{attempt.plannedAgentName}</p><p>AI connection: {attempt.provider} · <time dateTime={new Date(attempt.time * 1000).toISOString()}>{new Date(attempt.time * 1000).toLocaleString()}</time></p>
    <details onToggle={event => { setOpen(event.currentTarget.open); if (!event.currentTarget.open) setConfirmRemove(false); }}><summary ref={savedDisclosure}>Show saved details</summary>
      {open && <>
        {error && <p role="alert">{error}</p>}
        {!detail && !error && <p role="status">Reading saved details…</p>}
        {detail && <>
          {detail.interrupted && <p role="status">Last seen running; current result unknown</p>}
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
              <button type="button" className="workspace-secondary send-history-remove" disabled={busy} onClick={() => { void act('send_history_remove'); }}>Remove local copy</button>
              <button ref={keepDetails} type="button" className="workspace-secondary" disabled={busy} onClick={() => setConfirmRemove(false)}>Keep saved details</button>
            </> : <>
              {detail.canCheck && <button type="button" className="workspace-secondary" disabled={busy} onClick={() => { void act('send_history_check'); }}>Check cluster status</button>}
              {detail.canRemove && <button ref={removeInvoker} type="button" className="workspace-secondary send-history-remove" disabled={busy} onClick={() => setConfirmRemove(true)}>Remove saved details</button>}
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
      </dl></Details>
    </details>
  </li>;
}
export function SendHistory({ revision }: { revision: string }) {
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
  return <section className="send-history" aria-label="Past Sends">
    <h4>Past Sends</h4>
    <p>Earlier Sends and their test results. An interrupted Send may still be running on the cluster.</p>
    <Details hint="Saved results and limits"><p>Up to 256 Sends, 64 MiB total and 4 MiB per Send. Old terminal details are evicted first. Interrupted Sends are not resumed. Checking their status never sends tests again. Removing saved details changes only this computer, not the cluster. Removed or evicted answers may not be recoverable.</p></Details>
    {error ? <><p role="alert">{error}</p><button type="button" className="workspace-secondary" onClick={() => setRetry(v => v + 1)}>Retry saved Sends</button></>
      : history ? history.length ? <ol>{history.map(attempt => <SavedSend key={attempt.id} attempt={attempt} revision={revision} />)}</ol> : <p>No saved Sends yet.</p> : <p role="status">Reading saved Sends…</p>}
  </section>;
}
export default function Send({ documentId = SAMPLE_DOCUMENT_ID, place, source, model, local, stale, blocked, visible, remote, onChooseCluster, onReloadSource }: {
  documentId?: string; place: SelectedPlace; source?: SourceId; model?: string; local?: RunSnapshot; stale: boolean;
  blocked?: string; visible: boolean; remote: boolean; onChooseCluster?: () => void; onReloadSource?: () => Promise<void>;
}) {
  const [status, setSnapshot] = useState<SendStatus>();
  const snapshot = status?.documentId === documentId ? status.view : null;
  const foreignBusy = status?.documentId === documentId && status.busy?.documentId !== documentId ? status.busy : null;
  const [message, setMessage] = useState<string>();
  const [busy, setBusy] = useState(false);
  const admitted = useRef(false); const epoch = useRef(0);
  useEffect(() => {
    let mounted = true; let polling = false;
    admitted.current = false; setBusy(false); setMessage(undefined); setReloadError(undefined);
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
  const checkKey = JSON.stringify([documentId, targetId, providerId, agentHash, testsHash, refresh]);
  const checkingEnabled = (visible || remote) && !blocked && !!targetId && !!providerId && !!agentHash && !!testsHash;
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
    : !checked ? 'Checking this destination…' : checked.error ?? (mismatched ? 'Your saved files changed. Reload them to check this destination.'
    : !checked.value?.canSend ? 'Resolve the destination checks before sending.' : undefined));
  async function mutate(command: 'send_start' | 'send_stop') {
    if (admitted.current) return;
    let args: Record<string, unknown>;
    if (command === 'send_start') {
      if (reason || !target || !providerId || !checked?.value?.source) return;
      args = { targetId: target.id, providerId, expectedSource: checked.value.source };
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
  const running = snapshot?.state === 'running';
  const samePlace = snapshot?.targetId === target?.id && snapshot?.providerId === providerId;
  // A temporary reload block must not promote cached editor hashes to fresh evidence.
  const resultSource = remote && target && providerId ? checked?.value?.source ?? undefined : source;
  const editorCurrent = sameSource(source, snapshot?.source) === true;
  const current = !stale && editorCurrent && sameSource(resultSource, snapshot?.source) === true;
  const localCurrent = !stale && sameSource(source, local?.source) === true && sameSource(resultSource, local?.source) === true;
  const unconfirmed = !!snapshot?.source && !stale && editorCurrent && !resultSource;
  const targetName = snapshot?.targetName ?? (snapshot?.targetId === target?.id ? target?.context_name : undefined);
  const outdated = !!snapshot && (!samePlace || (!!snapshot.source && !current));
  const revisionExplanation = snapshot?.source && !current ? changedFiles(editorCurrent ? resultSource : source, snapshot.source, stale) : undefined;
  const uncertain = snapshot?.state === 'unknown' || snapshot?.report?.verdict === 'unknown';
  const resultState: Status = outdated ? 'stale' : running || busy || (checkingEnabled && !checked) ? 'running' : uncertain ? 'not-started' : snapshot?.report?.verdict === 'passed' ? 'passed' : snapshot?.report?.verdict === 'failed' || snapshot?.error ? 'failed' : reason ? 'needs-you' : 'not-started';
  const stages = { capture: 'Reading your saved files', access: 'Confirming cluster access', preflight: 'Checking the AI connection and tools', storage: 'Saving the Send summary', submit: 'Starting your tests', observe: 'Waiting for answers and evaluating tests', stopped: 'Stopped watching', done: 'Tests finished' };
  return <><section tabIndex={-1} className="send-panel" aria-label="Send and cluster tests" hidden={!visible && !remote && !running}>
    <LongCard title={visible ? 'Send your agent' : 'Tests on your cluster'} status={<StatusPill state={resultState}>{outdated ? unconfirmed ? 'Revision not confirmed' : undefined : running || resultState === 'running' ? undefined : uncertain ? snapshot?.stage === 'stopped' ? 'Stopped watching' : 'Result unknown' : resultLabel(snapshot?.report)}</StatusPill>} summary={<>The saved agent and test files go together{snapshot?.model && <> · <code>{snapshot.model}</code></>}{targetName && <> · <code>{targetName}</code></>}. Each Send creates a new agent, kept on the cluster even if a test fails.</>}
      action={<div className="workspace-step-actions">{!running && <button type="button" className="setup-button" disabled={!!reason || busy || !!message} title={reason} onClick={() => { void mutate('send_start'); }}>{visible ? 'Send and run tests' : 'Run all tests'}</button>}
      {running && <button type="button" className="workspace-secondary" disabled={busy} onClick={() => { void mutate('send_stop'); }}>Stop</button>}
    </div>}>
    <p className="sr-only test-announcement" role="status" aria-live="polite" aria-atomic="true">{!running ? outdated ? revisionExplanation : snapshot?.stage === 'stopped' ? 'Stopped watching.' : uncertain ? 'Test result unknown.' : resultAnnouncement(snapshot?.report) : undefined}</p>
    <CardSection title="What you send">
    <dl className="cluster-details"><div><dt>Source</dt><dd>Your saved agent and test files</dd></div>
      <div><dt>This computer</dt><dd>{local?.report ? localCurrent ? `Tests: ${words[local.report.verdict]}` : 'Changed since local tests, or revision not confirmed' : 'No local test result'}</dd></div>
      <div><dt>Destination</dt><dd>{target ? <><code>{target.context_name}</code> · <code>{target.watched_namespace}</code></> : 'Choose a cluster'}</dd></div>
      {provider && <div><dt>AI connection</dt><dd>{(checked?.value?.model ?? model) && <><code>{checked?.value?.model ?? model}</code> through </>}<code>{provider.name}</code> · {provider.ready ? 'AI connection ready; the model is checked when tests run' : 'AI connection not ready'}</dd></div>}
    </dl>
    {(!target || !provider) && <button type="button" className="workspace-secondary" onClick={onChooseCluster} disabled={!onChooseCluster}>Choose a cluster</button>}
    {checkingEnabled && <section className="send-preflight" aria-label="Before Send" aria-busy={!checked}>
      <h4>Before Send</h4>
      {!mismatched && checked?.value && <ul>{checked.value.rows.map(row => <li key={`${row.label}-${row.message}`} data-state={row.status}>
        <span className="preflight-state"><StatusIcon state={row.status === 'allowed' ? 'passed' : row.status === 'blocked' ? 'needs-you' : row.status === 'cannotCheck' ? 'stale' : 'not-started'} />{{ allowed: 'Ready', blocked: 'Blocked', cannotCheck: 'Can’t check', unverified: 'Unverified' }[row.status]}</span>
        <div><b>{row.label}</b><p>{row.message}</p></div>
      </li>)}</ul>}
      {reason && <p role={checked ? 'alert' : 'status'}>{reason}</p>}
      {!checked && <Elapsed />}
      {checked && (mismatched ? <button type="button" className="workspace-secondary" disabled={busy || running || !onReloadSource} onClick={() => {
        setReloadError(undefined);
        void onReloadSource?.().catch(() => setReloadError('Your saved files could not be reloaded. Your draft is still here.'));
      }}>Reload saved files</button> : <button type="button" className="workspace-secondary" disabled={busy || running} onClick={() => setRefresh(v => v + 1)}>Check again</button>)}
      {reloadError && <p role="alert">{reloadError}</p>}
    </section>}
    </CardSection>
    <CardSection title="What happened">
    {foreignBusy && <p role="status">{foreignBusy.name} is sending tests.</p>}
    {message && <p role="alert">{message}</p>}
    {snapshot?.storageError && snapshot.error !== 'storage' && <p role="alert">Bridge could not save this Send. Its saved summary may be missing or unknown.</p>}
    {running && <TestProgress stage={stages[snapshot.stage]} />}
    {snapshot?.stage === 'stopped' && <p role="status">Stopped watching. The cluster may still be running checks.</p>}
    {snapshot?.error && snapshot.stage !== 'stopped' && <p role="alert">{sendErrors[snapshot.error as keyof typeof sendErrors]}</p>}
    {snapshot && !samePlace && <p role="status">This result belongs to another cluster or AI connection, not your current selection.</p>}
    {revisionExplanation && <p className="muted">{revisionExplanation}</p>}
    {outdated && uncertain && !snapshot?.report && <p>Earlier result unknown</p>}
    {!running && <TestResults report={snapshot?.report} stale={outdated} />}
    </CardSection>
    {snapshot && <Details hint="Resources and file hashes"><dl className="cluster-details"><div><dt>Result destination</dt><dd>{targetName ?? 'Not confirmed'}</dd></div>
      <div><dt>Model sent</dt><dd><code>{snapshot.model ?? 'Not captured'}</code></dd></div>
    </dl>
      {snapshot.agentName && <GeneratedValue label="Planned Agent" value={snapshot.agentName} />}
      {snapshot.source && <><GeneratedValue label="Agent file hash" value={snapshot.source.agent} /><GeneratedValue label="Test file hash" value={snapshot.source.tests} /></>}
      {snapshot.acknowledged.map(r => <GeneratedValue key={`${r.kind}-${r.uid}`} label={r.kind} value={r.name} />)}
      {snapshot.pending && <GeneratedValue label={`Possibly created ${snapshot.pending.kind}`} value={snapshot.pending.name} />}
      <p>Sending requires permission to create an agent and run its tests. Saved answers and test receipts remain in Past Sends on this computer, within its retention limits. Stop ends watching and prevents further submissions; it does not cancel checks already sent.</p>
    </Details>}
    </LongCard>
  </section><SendHistory revision={`${documentId}:${status?.busy?.documentId ?? ''}:${snapshot?.id ?? ''}:${snapshot?.state ?? ''}:${snapshot?.stage ?? ''}:${snapshot?.storageError ?? false}`} /></>;
}
