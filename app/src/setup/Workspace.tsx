import { useEffect, useState } from 'react';
import { Button } from '../components/Button';
import { StatusPill } from '../components/DesignSystem';
import { Disclosure } from '../components/Disclosure';
import { creationRoot, listDocuments, type AgentEntry } from '../designer/documents-api';
import type { ModelState } from './Model';
import { workspaceView } from './progress';
import Recovery from './Recovery';
import type { SetupStatus } from './state';

// Returning workspace management is not another pass through first-run setup.
export default function Workspace({ status, busy, stopping, starting, error, model, active, onStart, onCancel, onStop }: {
  status: SetupStatus; busy: boolean; stopping: boolean; starting: boolean; error?: string; model: ModelState; active: boolean;
  onStart: () => void; onCancel: () => void; onStop: (button: HTMLButtonElement) => void;
}) {
  const [agents, setAgents] = useState<AgentEntry[]>();
  const [path, setPath] = useState<string>();
  const [inside, setInside] = useState(false);
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
  const ready = view.word === 'Running';
  const elapsed = status.workspaceProgress?.elapsedMs;
  const seconds = elapsed === undefined ? undefined : Math.floor(elapsed / 1000);
  const clock = seconds === undefined ? undefined : `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
  return <section className="card returning-workspace" aria-label="Workspace">
    <div className="steps-head"><div className="workspace-title"><h2>Workspace</h2><StatusPill state={error ? 'needs-you' : view.status}>{error ? 'Status unavailable' : view.word}</StatusPill></div>
    {!failed && !closed && <div className="workspace-actions">
      {ready ? <Button type="button" kind="secondary" disabled={busy || !!error} onClick={event => onStop(event.currentTarget)}>Stop</Button>
        : view.word === 'Starting' ? <Button type="button" kind="secondary" disabled={busy || !!error} onClick={onCancel}>Cancel</Button>
          : <Button type="button" kind="primary" disabled={busy || !!error || view.word === 'Stopping'} onClick={onStart}>Start</Button>}
    </div>}</div>
    <p role="status">{error ? 'Workspace status is unavailable. Check again before starting it.' : view.detail}</p>
    {view.word === 'Stopping' && <p className="muted">Start is available once it has stopped.</p>}
    {view.word === 'Starting' && clock && <p className="muted">{clock} so far</p>}
    {closed && <p>Quit Bridge and reopen it to start the workspace.</p>}
    {failed && <Recovery detail={status.localVmError} preflightFailure={status.preflightFailure} failure={status.workspaceFailure}
      disabled={busy || !!error || status.workspaceFailure?.operation === 'stop'} onRetry={onStart} />}
    <dl className="workspace-facts">
      <div><dt>AI</dt><dd>{ready ? model === 'connected' ? 'Connected' : 'Connect the AI below to run your agents.' : 'Connect the AI after starting the workspace.'}</dd></div>
      <div><dt>Agents</dt><dd>{agents ? agents.length ? `${agents[0].name}${agents.length > 1 ? ` and ${agents.length - 1} more` : ''}` : 'No saved agents yet' : 'Agent list unavailable'}</dd></div>
      <div><dt>Files</dt><dd>{path ? <code>{path}</code> : 'Folder unavailable'}</dd></div>
    </dl>
    <Disclosure label="What's inside" open={inside} controls="returning-workspace-inside" onOpenChange={setInside}>
      {inside && <p id="returning-workspace-inside">The workspace includes Orka and the tools your agents use. Stopping keeps your agents and their files.</p>}
    </Disclosure>
  </section>;
}
