import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Button } from '../components/Button';
import { StatusPill } from '../components/DesignSystem';
import { Disclosure } from '../components/Disclosure';
import { type AgentEntry, creationRoot, listDocuments } from '../designer/documents-api';
import { workspaceView } from './progress';
import { workspaceRecovery } from './recovery-info';
import { preflightRecovery } from './preflight-state';
import type { SetupStatus } from './state';

// Returning workspace management is not another pass through first-run setup.
export default function Workspace({ status, busy, stopping, starting, error, active, onStart, upgrade, updating, updateBlocked, ordinaryRecovery = !upgrade }: {
  upgrade?: ReactNode; updating?: boolean; updateBlocked?: boolean; ordinaryRecovery?: boolean;
  status: SetupStatus; busy: boolean; stopping: boolean; starting: boolean; error?: string; active: boolean;
  onStart: () => void;
}) {
  const [agents, setAgents] = useState<AgentEntry[]>();
  const [path, setPath] = useState<string>();
  const [inside, setInside] = useState(false);
  const [details, setDetails] = useState(false);
  const [quitError, setQuitError] = useState(false);
  const [quitting, setQuitting] = useState(false);
  const [helpError, setHelpError] = useState(false);
  const helpGeneration = useRef(0);
  async function quit() {
    setQuitting(true); setQuitError(false);
    try { await invoke('quit_bridge'); } catch { setQuitError(true); setQuitting(false); }
  }
  useEffect(() => {
    if (!active) return;
    let current = true;
    setAgents(undefined); setPath(undefined);
    void listDocuments().then(value => { if (current) setAgents(value); });
    void creationRoot().then(value => { if (current) setPath(value); });
    return () => { current = false; };
  }, [active]);
  const view = workspaceView(status, { stopping, starting });
  const failed = status.localVm === 'failed' && !stopping && !starting;
  const closed = status.localVm === 'closed';
  const stopped = status.localVm === 'cancelled';
  const stopFailed = failed && status.workspaceFailure?.operation === 'stop';
  const forcedStop = stopFailed && status.workspaceFailure?.cause === 'forcedStop';
  const recovery = status.preflightFailure ? preflightRecovery(status.preflightFailure) : workspaceRecovery(status.workspaceFailure);
  useEffect(() => {
    helpGeneration.current += 1;
    setHelpError(false);
  }, [status.localVm, status.preflightFailure, status.workspaceFailure?.cause, status.workspaceFailure?.operation]);
  async function help() {
    const generation = helpGeneration.current;
    try { await invoke('setup_open_help', { topic: recovery.topic }); if (generation === helpGeneration.current) setHelpError(false); }
    catch { if (generation === helpGeneration.current) setHelpError(true); }
  }
  const elapsed = status.workspaceProgress?.elapsedMs;
  const seconds = elapsed === undefined ? undefined : Math.floor(elapsed / 1000);
  const clock = seconds === undefined ? undefined : `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
  return <section className="card returning-workspace" aria-label="Workspace">
    <div className="steps-head"><div className="workspace-title"><h2>Workspace</h2><StatusPill state={error ? 'needs-you' : updating ? 'running' : updateBlocked ? 'needs-you' : view.status}>{error ? 'Status unavailable' : updating ? 'Updating' : updateBlocked ? 'Needs you' : view.word}</StatusPill></div>
    </div>
    {(!upgrade || error || stopFailed || ordinaryRecovery && (failed || stopped)) && <p role="status">{error ? 'Workspace status is unavailable. Check again before starting it.' : updating ? 'Updating the workspace. Your agent files are kept.' : stopFailed && !forcedStop ? "The workspace didn't stop." : failed ? recovery.summary : stopped ? 'The workspace stopped unexpectedly. Your agents and files are kept.' : view.detail}</p>}
    {forcedStop && <><p>{recovery.preservation}</p><p>{recovery.action}</p></>}
    {view.word === 'Starting' && clock && <p className="muted">{clock} so far</p>}
    {upgrade}
    {(stopFailed || closed) && <Button type="button" kind="primary" disabled={quitting} onClick={() => { void quit(); }}>Quit Bridge</Button>}
    {failed && ordinaryRecovery && !stopFailed && recovery.action && <p>{recovery.action.replaceAll('Try again', 'Restart workspace').replaceAll('copy the details for help', 'open Show details for help')}</p>}
    {ordinaryRecovery && (failed || stopped) && !stopFailed && <div className="workspace-step-actions"><Button type="button" kind="primary" disabled={busy || !!error || failed && recovery.retryBlocked} title={failed && recovery.retryBlocked ? recovery.action : undefined} onClick={onStart}>Restart workspace</Button>
      {failed && recovery.topic && <Button type="button" kind="secondary" onClick={() => { void help(); }}>Show me how</Button>}</div>}
    {failed && helpError && <p role="alert">Couldn't open help. Open Show details to read the error.</p>}
    {quitError && <p role="alert">Couldn't quit Bridge. Try again or use Quit Bridge from its menu.</p>}
    {failed && ordinaryRecovery && <Disclosure label={details ? 'Hide details' : 'Show details'} open={details} controls="returning-workspace-error" onOpenChange={setDetails}>
      {details && <div className="workspace-progress-details"><pre id="returning-workspace-error">{status.localVmError || recovery.summary}</pre></div>}
    </Disclosure>}
    <dl className="workspace-facts">
      <div><dt>Agents</dt><dd>{agents ? agents.length ? `${agents[0].name}${agents.length > 1 ? ` and ${agents.length - 1} more` : ''}` : 'No saved agents yet' : 'Agent list unavailable'}</dd></div>
      <div><dt>Files</dt><dd>{path ? <code>{path}</code> : 'Folder unavailable'}</dd></div>
    </dl>
    <Disclosure label="What's inside" open={inside} controls="returning-workspace-inside" onOpenChange={setInside}>
      {inside && <p id="returning-workspace-inside">The workspace includes Orka and the tools your agents use. Uses about 4 GB of memory while Bridge is open. Quit Bridge to free it.</p>}
    </Disclosure>
  </section>;
}
