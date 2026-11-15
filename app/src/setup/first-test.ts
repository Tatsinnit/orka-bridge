import { useEffect, useRef, useState } from 'react';
import { agentCall, SAMPLE_DOCUMENT_ID } from '../designer/agent-api';
import { runCall, type RunSnapshot } from '../designer/run-api';
import type { CheckStepState } from './Steps';
import { loadExample } from './state';

const preference = 'bridge.firstTest.v1';
export function armFirstTest() {
  try { if (localStorage.getItem(preference) === null) localStorage.setItem(preference, 'pending'); } catch { /* No durable claim means no automatic side effect. */ }
}
export function retireFirstTest() {
  try { if (localStorage.getItem(preference) === 'pending') localStorage.setItem(preference, 'attempted'); } catch { /* An unreadable preference cannot admit a test. */ }
}
export function firstTestPending() {
  try { return localStorage.getItem(preference) === 'pending'; } catch { return false; }
}
function claim() {
  if (!firstTestPending()) return false;
  try { localStorage.setItem(preference, 'attempted'); return true; } catch { return false; }
}
const text = (value: string) => value.replaceAll('\r\n', '\n');

export function useFirstTest(options: { ready: boolean; canOpen: boolean; sampleDirty: boolean; onOpen: () => void; onPrepared?: () => void; onSnapshot?: (snapshot: RunSnapshot | undefined) => void }): CheckStepState | undefined {
  const latest = useRef(options);
  useEffect(() => { latest.current = options; });
  const [check, setCheck] = useState<CheckStepState>();
  const [prepareAttempt, setPrepareAttempt] = useState(0);
  const mounted = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; clearTimeout(timer.current); };
  }, []);
  useEffect(() => {
    if (!options.ready || options.sampleDirty || !firstTestPending()) return;
    let cancelled = false;
    let claimed = false;
    const finish = (snapshot?: RunSnapshot) => {
      if (cancelled || !mounted.current) return;
      latest.current.onSnapshot?.(snapshot);
      const verdict = snapshot?.state === 'complete' ? snapshot.report?.verdict ?? 'unknown' : 'unknown';
      setCheck(verdict === 'cancelled' ? 'unknown' : verdict);
      if (latest.current.ready && latest.current.canOpen) latest.current.onOpen();
    };
    const poll = () => {
      timer.current = setTimeout(() => { void runCall('agent_run_status').then(result => {
        if (cancelled || !mounted.current) return;
        if (result.ok) void observe(result.snapshot); else poll();
      }); }, 500);
    };
    const observe = (snapshot: RunSnapshot) => {
      if (cancelled || !mounted.current) return;
      if (snapshot.state !== 'running' && snapshot.busy?.documentId !== SAMPLE_DOCUMENT_ID) { finish(snapshot); return; }
      latest.current.onSnapshot?.(snapshot);
      setCheck('running');
      poll();
    };
    // Preparation can update only the pristine managed sample. Finish it before reading saved bytes.
    void loadExample(true).then(async bundled => {
      if (cancelled || !mounted.current || !latest.current.ready || latest.current.sampleDirty) return;
      if (!bundled.ok && bundled.retryable) {
        timer.current = setTimeout(() => { if (!cancelled && mounted.current) setPrepareAttempt(value => value + 1); }, 500);
        return;
      }
      if (bundled.ok) latest.current.onPrepared?.();
      const [saved, status] = await Promise.all([agentCall('agent_open', undefined, SAMPLE_DOCUMENT_ID), runCall('agent_run_status')]);
      // StrictMode's abandoned effect must not consume the durable one-shot.
      if (cancelled || !mounted.current || !latest.current.ready || latest.current.sampleDirty || !claim()) return;
      claimed = true;
      if (!bundled.ok || text(saved.agent) !== text(bundled.pair.agent) || text(saved.tests) !== text(bundled.pair.tests) || !status.ok) { finish(); return; }
      if (status.snapshot.state !== 'idle') { void observe(status.snapshot); return; }
      if (!status.snapshot.canRun || status.snapshot.busy) { finish(); return; }
      // Claim is persisted before submission: an interrupted/unknown attempt is never replayed.
      setCheck('running');
      const result = await runCall('agent_run', SAMPLE_DOCUMENT_ID, true);
      if (cancelled || !mounted.current) return;
      // A lost reply may still own native busy; reconcile read-only until release.
      if (result.ok) void observe(result.snapshot); else poll();
    }).catch(() => { if (!cancelled && mounted.current && claim()) finish(); });
    // Eligibility gates submission, not observation of an already claimed attempt.
    // A transient setup poll failure must not orphan native busy ownership.
    return () => { if (!claimed) cancelled = true; };
  }, [options.ready, options.sampleDirty, prepareAttempt]);
  return check;
}
