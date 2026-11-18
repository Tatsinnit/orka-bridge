import { Disclosure } from '../components/Disclosure';
import { Button } from '../components/Button';
import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { helpDetails, workspaceRecovery, type WorkspaceFailure } from './recovery-info';
import { preflightRecovery, type PreflightFailure } from './preflight-state';

export default function Recovery({ detail, preflightFailure, failure, disabled, onRetry, onCluster }: { detail?: string | null; preflightFailure?: PreflightFailure; failure?: WorkspaceFailure; disabled: boolean; onRetry: () => void; onCluster?: () => void }) {
  const [expanded, setExpanded] = useState(false);
  const [notice, setNotice] = useState('');
  const [quitting, setQuitting] = useState(false);
  const info = preflightFailure ? { ...preflightRecovery(preflightFailure), category: preflightFailure } : workspaceRecovery(failure);
  const forcedStop = info.category === 'forcedStop';
  async function quit() {
    setQuitting(true); setNotice('');
    try { await invoke('quit_bridge'); }
    catch { setNotice("Couldn't quit Bridge. Use Quit Bridge from its menu."); setQuitting(false); }
  }
  async function help() {
    try { await invoke('setup_open_help', { topic: info.topic }); setNotice('Help opened in your browser.'); }
    catch { setNotice("Couldn't open help. Open Show details and ask for help with this error."); }
  }
  async function copy() {
    try { await navigator.clipboard.writeText(helpDetails(info, failure?.codes)); setNotice('Details copied. Private diagnostic text is not included.'); }
    catch { setNotice("Couldn't copy. Open Show details to read the error."); }
  }
  return <div className="workspace-failure" role="alert">
    <div className="workspace-failure-explanation"><p className="failure-summary">{info.summary}</p>
    {info.action && <p>{info.action}</p>}{(preflightFailure || info.preservation) && <p>{preflightFailure ? 'This check made no changes.' : info.preservation}</p>}
    <div className="workspace-step-actions">
      {forcedStop && <Button type="button" kind="primary" disabled={disabled || quitting} onClick={() => { void quit(); }}>{quitting ? 'Quitting Bridge…' : 'Quit Bridge'}</Button>}
      {info.topic && <Button type="button" kind={forcedStop ? 'secondary' : 'primary'} onClick={() => { void help(); }}>Show me how</Button>}
      {!forcedStop && <Button type="button" kind={info.topic ? 'secondary' : 'primary'} disabled={disabled || info.retryBlocked}
        title={info.retryBlocked ? info.category === 'stopUnconfirmed' ? 'Quit Bridge and reopen it before trying again' : info.action : undefined} onClick={onRetry}>Try again</Button>}
      <Button type="button" kind="secondary" onClick={() => { void copy(); }}>Copy details for help</Button>
    </div>
    <Disclosure label={expanded ? 'Hide details' : 'Show details'} open={expanded} controls="workspace-failure-details" onOpenChange={setExpanded}>
    {expanded && <div id="workspace-failure-details" className="workspace-progress-details"><p>Stage: {info.stage}</p><pre tabIndex={0}>{preflightFailure ? info.summary : detail || 'No further error details were reported.'}</pre></div>}</Disclosure>
    </div>
    {onCluster && <Button kind="secondary" className="workspace-fallback" disabled={disabled} onClick={onCluster}>Use a cluster you already have instead</Button>}
    <p className="muted" role="status">{notice}</p>
  </div>;
}
