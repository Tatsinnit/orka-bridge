import { Button } from '../components/Button';
import { useEffect, useRef, useState } from 'react';
import { CardSection, Details, GeneratedValue, LongCard, StatusPill, type Status } from '../components/DesignSystem';
import { changedFiles, resultAnnouncement, resultLabel, TestProgress, TestResults } from '../components/TestResults';
import { SAMPLE_DOCUMENT_ID, type SourceId, sameSource } from './agent-api';
import { errors, type RunSnapshot, runCall } from './run-api';
export default function Run({ documentId = SAMPLE_DOCUMENT_ID, onSnapshot, blocked = false, stale = false, onStart, source }: { documentId?: string; onSnapshot?: (snapshot: RunSnapshot | undefined) => void; blocked?: boolean; stale?: boolean; onStart?: () => void; source?: SourceId }) {
  const [received, setSnapshot] = useState<RunSnapshot>();
  const snapshot = received?.documentId === documentId ? received : undefined;
  const [message, setMessage] = useState<string>();
  const [sending, setSending] = useState(false);
  const epoch = useRef(0);
  const sourceCurrent = sameSource(source, snapshot?.source);
  useEffect(() => {
    const request = ++epoch.current;
    setMessage(undefined); setSending(false);
    const observe = async () => {
      const result = await runCall('agent_run_status', documentId);
      if (request !== epoch.current) return;
      if (result.ok) { setSnapshot(result.snapshot); setMessage(undefined); } else setMessage(result.message);
    };
    void observe(); const timer = window.setInterval(() => { void observe(); }, 500);
    return () => { epoch.current++; window.clearInterval(timer); };
  }, [documentId]);
  useEffect(() => { if (snapshot || message) onSnapshot?.(message ? undefined : snapshot ? { ...snapshot, sourceCurrent } : undefined); }, [snapshot, message, onSnapshot, sourceCurrent]);
  const start = async () => {
    if (sending || blocked || !snapshot?.canRun) return;
    setSending(true);
    const request = epoch.current;
    const result = await runCall('agent_run', documentId);
    if (request !== epoch.current) return;
    if (result.ok) { onStart?.(); setSnapshot(result.snapshot); setMessage(undefined); } else setMessage(result.message);
    setSending(false);
  };
  const running = sending || snapshot?.state === 'running';
  const outdated = stale || (sourceCurrent === false && !!snapshot?.report);
  const uncertain = snapshot?.state === 'unknown' || snapshot?.report?.verdict === 'unknown';
  const failed = snapshot?.error && !(snapshot.error === 'model' && snapshot.modelConnected);
  const state: Status = outdated ? 'stale' : running ? 'running' : uncertain ? 'stale' : failed ? 'failed' : snapshot?.report?.verdict === 'passed' ? 'passed' : snapshot?.report?.verdict === 'failed' ? 'failed' : 'not-started';
  const stage = { capture: 'Reading your saved files', model: 'Checking the local model', submit: 'Starting your tests', observe: 'Waiting for answers and evaluating tests', cancel: 'Cancelling tests', done: 'Tests finished' };
  return <section className="example-run" aria-label="Tests on the local cluster">
    <LongCard title="Tests on the local cluster" status={<><StatusPill state={state} /><span className="status-description">{outdated ? 'Not run since your edit' : running ? undefined : uncertain ? 'Result unknown' : resultLabel(snapshot?.report)}</span></>} summary={<>On this computer{snapshot?.model && <> · <code>{snapshot.model}</code></>}.{snapshot?.modelConnected === false && ' Connect the AI in Clusters first.'}</>}
      action={!running && <Button type="button" kind="primary" disabled={blocked || !snapshot?.canRun} title={blocked ? 'Save your changes before running the test' : !snapshot?.canRun ? 'Start the local cluster and connect the AI in Clusters first' : undefined} onClick={() => { void start(); }}>Run tests</Button>}>
      <p className="sr-only test-announcement" role="status" aria-live="polite" aria-atomic="true">{!running && !outdated ? uncertain ? 'Test result unknown.' : resultAnnouncement(snapshot?.report) : undefined}</p>
      <CardSection title="What happened">
        {snapshot?.busy && snapshot.busy.documentId !== documentId && <p role="status">{snapshot.busy.name} is running a test.</p>}
        {outdated && <p role="status">{changedFiles(source, snapshot?.source, stale)}</p>}
        {running && <TestProgress stage={snapshot?.stage && snapshot.stage !== 'done' ? stage[snapshot.stage] : 'Starting your tests'} />}
        {message && <p role="status">{message}</p>}
        {snapshot?.error && !(snapshot.error === 'model' && snapshot.modelConnected) && <p role="alert">{errors[snapshot.error]}</p>}
        {!running && <TestResults report={snapshot?.report} stale={outdated} />}
      </CardSection>
      {(!!snapshot?.acknowledged.length || snapshot?.pending || snapshot?.source) && <Details hint="Resources and file hashes">
        {snapshot.source && <><GeneratedValue label="Agent file hash" value={snapshot.source.agent} /><GeneratedValue label="Test file hash" value={snapshot.source.tests} /></>}
        {snapshot.acknowledged.map(r => <GeneratedValue key={r.uid} label={r.kind} value={r.name} />)}
        {snapshot.pending && <GeneratedValue label={`Possibly created ${snapshot.pending.kind}`} value={snapshot.pending.name} />}
      </Details>}
    </LongCard>
  </section>;
}
