import { Button } from '../components/Button';
import { useEffect, useRef, useState } from 'react';
import { CardSection, Details, GeneratedValue, LongCard, StatusPill, type Status } from '../components/DesignSystem';
import { changedFiles, resultAnnouncement, resultLabel, TestProgress, TestResults } from '../components/TestResults';
import { SAMPLE_DOCUMENT_ID, type SourceId, sameSource } from './agent-api';
import { errors, type RunSnapshot, runCall } from './run-api';
export default function Run({ documentId = SAMPLE_DOCUMENT_ID, onSnapshot, blocked = false, stale = false, onStart, source, onStopLocalCluster }: { documentId?: string; onSnapshot?: (snapshot: RunSnapshot | undefined) => void; blocked?: boolean; stale?: boolean; onStart?: () => void; source?: SourceId; onStopLocalCluster?: () => void }) {
  const [received, setSnapshot] = useState<RunSnapshot>();
  const snapshot = received?.documentId === documentId ? received : undefined;
  const [message, setMessage] = useState<string>();
  const [action, setAction] = useState<'agent_run' | 'agent_run_cancel' | 'agent_run_check_cancellation'>();
  const [actionMessage, setActionMessage] = useState<string>();
  const admitted = useRef(false);
  const observations = useRef(0);
  const epoch = useRef(0);
  const sourceCurrent = sameSource(source, snapshot?.source);
  useEffect(() => {
    const request = ++epoch.current;
    setMessage(undefined); setAction(undefined); setActionMessage(undefined); admitted.current = false;
    let polling = false;
    const observe = async () => {
      if (admitted.current || polling) return;
      polling = true;
      const observation = ++observations.current;
      const result = await runCall('agent_run_status', documentId);
      polling = false;
      if (request !== epoch.current || observation !== observations.current || admitted.current) return;
      if (result.ok) { setSnapshot(result.snapshot); setMessage(undefined); } else setMessage(result.message);
    };
    void observe(); const timer = window.setInterval(() => { void observe(); }, 500);
    return () => { epoch.current++; window.clearInterval(timer); };
  }, [documentId]);
  useEffect(() => { if (snapshot || message) onSnapshot?.(message ? undefined : snapshot ? { ...snapshot, sourceCurrent } : undefined); }, [snapshot, message, onSnapshot, sourceCurrent]);
  const act = async (command: NonNullable<typeof action>) => {
    if (admitted.current || !snapshot || command === 'agent_run' && (blocked || !snapshot.canRun)
      || command === 'agent_run_cancel' && !(snapshot.canCancel || snapshot.canRetryCancellation)
      || command === 'agent_run_check_cancellation' && !snapshot.canCheckCancellation) return;
    admitted.current = true; observations.current++; setAction(command); setActionMessage(undefined);
    const request = epoch.current;
    const result = await runCall(command, documentId);
    if (request !== epoch.current) return;
    if (result.ok) { if (command === 'agent_run') onStart?.(); setSnapshot(result.snapshot); setMessage(undefined); }
    else setActionMessage(result.message);
    observations.current++; admitted.current = false; setAction(undefined);
  };
  const cancelling = action === 'agent_run_cancel' || action === 'agent_run_check_cancellation' || snapshot?.cancellation === 'cancelling';
  const running = action === 'agent_run' || cancelling || snapshot?.state === 'running';
  const unconfirmed = snapshot?.cancellation === 'unconfirmed';
  const beforeStart = snapshot?.cancellation === 'beforeStart';
  const confirmed = snapshot?.cancellation === 'cancelled';
  const outdated = stale || (sourceCurrent === false && !!snapshot?.report);
  const uncertain = snapshot?.state === 'unknown' || snapshot?.report?.verdict === 'unknown';
  const failed = snapshot?.error && !(snapshot.error === 'model' && snapshot.modelConnected);
  const state: Status = outdated ? 'stale' : running ? 'running' : unconfirmed ? 'needs-you' : confirmed || beforeStart ? 'not-started' : uncertain ? 'stale' : failed ? 'failed' : snapshot?.report?.verdict === 'passed' ? 'passed' : snapshot?.report?.verdict === 'failed' ? 'failed' : 'not-started';
  const counts = snapshot?.report?.cases;
  const unknownCount = counts?.filter(c => c.verdict === 'unknown').length ?? 0;
  const countLine = counts ? `${counts.filter(c => c.verdict === 'passed').length} passed, ${counts.filter(c => c.verdict === 'failed').length} failed, ${counts.filter(c => c.verdict === 'cancelled').length} cancelled${unknownCount ? `, ${unknownCount} unknown` : ''}.` : '';
  const cancellationLine = beforeStart ? 'Cancelled before any test started.' : unconfirmed ? 'Cancellation not confirmed. Work may still be running on this computer.'
    : confirmed ? `${counts?.some(c => c.verdict === 'cancelled') ? 'Tests cancelled.' : 'Tests finished before cancellation.'} The test worker on this computer has stopped. ${countLine}` : undefined;
  const stage = { capture: 'Reading your saved files', model: 'Checking the local model', submit: 'Starting your tests', observe: 'Waiting for answers and evaluating tests', cancel: 'Cancelling tests', done: 'Tests finished' };
  return <section className="example-run" aria-label="Tests on the local cluster">
    <LongCard title="Tests on the local cluster" status={<><StatusPill state={state}>{outdated ? undefined : cancelling ? 'Cancelling' : unconfirmed ? 'Not confirmed' : confirmed || beforeStart ? 'Cancelled' : undefined}</StatusPill><span className="status-description">{outdated ? 'Not run since your edit' : running || cancellationLine ? undefined : uncertain ? 'Result unknown' : resultLabel(snapshot?.report)}</span></>} summary={<>On this computer{snapshot?.model && <> · <code>{snapshot.model}</code></>}.{snapshot?.modelConnected === false && ' Connect the AI in Clusters first.'}</>}
      action={running ? !cancelling && snapshot?.canCancel && <Button type="button" kind="secondary" onClick={() => { void act('agent_run_cancel'); }}>Cancel tests</Button>
        : (!unconfirmed || !snapshot?.canCheckCancellation) && <Button type="button" kind="primary" disabled={!!action || blocked || !snapshot?.canRun} title={blocked ? 'Save your changes before running the test' : !snapshot?.canRun ? 'Start the local cluster and connect the AI in Clusters first' : undefined} onClick={() => { void act('agent_run'); }}>Run tests</Button>}>
      <p className="sr-only test-announcement" role="status" aria-live="polite" aria-atomic="true">{!running ? cancellationLine ?? (!outdated ? uncertain ? 'Test result unknown.' : resultAnnouncement(snapshot?.report) : undefined) : undefined}</p>
      <CardSection title="What happened">
        {snapshot?.busy && snapshot.busy.documentId !== documentId && <p role="status">{snapshot.busy.name} is running a test.</p>}
        {outdated && <p role="status">{changedFiles(source, snapshot?.source, stale)}</p>}
        {running && <TestProgress stage={cancelling ? 'Cancelling tests · waiting for the test worker to stop' : snapshot?.stage && snapshot.stage !== 'done' ? stage[snapshot.stage] : 'Starting your tests'} />}
        {running && !cancelling && snapshot?.canCancel && <p>Cancel stops the tests on this computer and keeps answers that already finished.</p>}
        {!running && cancellationLine && <p>{cancellationLine}</p>}
        {!running && unconfirmed && <><p>Stop the local cluster to stop all work on this computer. Actions already taken are not undone.</p>
          <div className="workspace-step-actions">
            {snapshot?.canRetryCancellation && <Button type="button" kind="primary" onClick={() => { void act('agent_run_cancel'); }}>Retry cancellation</Button>}
            {snapshot?.canCheckCancellation && <Button type="button" kind="quiet" onClick={() => { void act('agent_run_check_cancellation'); }}>Check again</Button>}
            <Button type="button" kind="quiet" onClick={onStopLocalCluster}>Stop the local cluster</Button>
          </div>
        </>}
        {actionMessage && <p role="status">{actionMessage}</p>}
        {message && <p role="status">{message}</p>}
        {snapshot?.error && !(snapshot.error === 'model' && snapshot.modelConnected) && <p role="alert">{errors[snapshot.error]}</p>}
        {(!running || cancelling) && <TestResults report={snapshot?.report} stale={outdated} />}
      </CardSection>
      {(!!snapshot?.acknowledged.length || snapshot?.pending || snapshot?.source) && <Details hint="Resources and file hashes">
        {snapshot.source && <><GeneratedValue label="Agent file hash" value={snapshot.source.agent} /><GeneratedValue label="Test file hash" value={snapshot.source.tests} /></>}
        {snapshot.acknowledged.map(r => <GeneratedValue key={r.uid} label={r.kind} value={r.name} />)}
        {snapshot.pending && <GeneratedValue label={`Possibly created ${snapshot.pending.kind}`} value={snapshot.pending.name} />}
      </Details>}
    </LongCard>
  </section>;
}
