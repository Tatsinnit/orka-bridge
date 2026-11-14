import { Button } from '../components/Button';
import { useEffect, useRef, useState } from 'react';
import { StatusPill } from '../components/DesignSystem';
import { connectModel, removeModel, type ModelFailure } from './modelState';
import { invoke } from '@tauri-apps/api/core';
import type { HelpTopic } from './recovery-info';

export type ModelState = 'idle' | 'queued' | 'connecting' | 'removing' | 'connected' | 'failed';
// `enabled` is native permission to touch credentials. `queueable` only permits remembering the
// creator's choice while the workspace is still arriving. `away` means another place was chosen.
export default function Model({ enabled, queueable = false, paused = false, away = false, primary = true, onState, onSettled, onConnectRequest }: {
  enabled: boolean; queueable?: boolean; paused?: boolean; away?: boolean; primary?: boolean; onState?: (state: ModelState) => void; onSettled?: () => void; onConnectRequest?: () => void;
}) {
  // In-memory consent only. It never carries a credential and is consumed once.
  const [queued, setQueued] = useState(false);
  const root = useRef<HTMLElement>(null);
  const status = useRef<HTMLDivElement>(null);
  const retry = useRef<HTMLButtonElement>(null);
  const removal = useRef<HTMLButtonElement>(null);
  const cancelRemoval = useRef<HTMLButtonElement>(null);
  const [confirmRemoval, setConfirmRemoval] = useState(false);
  // Focus follows the keyboard user's operation, then its retry or next available action.
  const refocus = useRef<'connect' | 'remove'>(undefined);
  const [busy, setBusy] = useState<'connect' | 'remove'>();
  const [removed, setRemoved] = useState(false);
  const [connected, setConnected] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const ready = enabled && connected;
  const hasSignIn = !removed && attempted;
  const [error, setError] = useState<ModelFailure>();
  const [notice, setNotice] = useState('');
  const [failedAction, setFailedAction] = useState<'connect' | 'remove'>('connect');
  const epoch = useRef(0);
  const active = useRef(false);
  useEffect(() => {
    epoch.current++;
    active.current = false;
    if (!enabled) { setBusy(undefined); setConnected(false); setRemoved(false); setError(undefined); }
    return () => { epoch.current++; };
  }, [enabled]);
  useEffect(() => {
    onState?.(!enabled ? queued ? 'queued' : 'idle' : busy === 'connect' ? 'connecting' : busy === 'remove' ? 'removing' : ready ? 'connected' : error ? 'failed' : queued ? 'queued' : 'idle');
  }, [enabled, busy, ready, error, queued, onState]);
  useEffect(() => { if (away) setQueued(false); }, [away]);
  // Runs after the enabled reset above. An unreadable observation or workspace mutation holds the intent.
  useEffect(() => {
    if (queued && enabled && !paused && !away) { setQueued(false); void perform('connect'); }
  }, [queued, enabled, paused, away]);
  // Busy and queued states replace the action with status; keep keyboard focus with the operation.
  // Focus is restored only while the operation still owns it: on the page (not hidden), and on nothing,
  // the status or its own action. Anywhere else the creator moved it, so drop the intent and leave focus alone.
  useEffect(() => {
    if (!refocus.current) return;
    const at = document.activeElement;
    const owns = !root.current?.closest('[hidden]') && (!at || at === document.body || at === status.current || at === retry.current || at === removal.current || at === cancelRemoval.current);
    if (!owns) { refocus.current = undefined; return; }
    if (busy || queued) { status.current?.focus(); return; }
    if (confirmRemoval) { cancelRemoval.current?.focus(); return; }
    (refocus.current === 'remove' ? removal.current ?? retry.current : retry.current)?.focus();
    if (document.activeElement === document.body) status.current?.focus();
    refocus.current = undefined;
  }, [busy, queued, ready, error, confirmRemoval]);
  async function perform(action: 'connect' | 'remove') {
    if (!enabled || paused || active.current || (action === 'connect' && ready)) return;
    active.current = true;
    const request = ++epoch.current;
    setBusy(action); setError(undefined); setRemoved(false); setNotice(''); setFailedAction(action);
    setAttempted(true); setConfirmRemoval(false);
    if (action === 'remove') setConnected(false);
    const result = await (action === 'connect' ? connectModel() : removeModel());
    if (request !== epoch.current) return;
    active.current = false; setBusy(undefined);
    if (result.ok) { setConnected(action === 'connect'); setRemoved(action === 'remove'); }
    else setError(result);
    onSettled?.();
  }
  async function help(topic: HelpTopic) {
    try { await invoke('setup_open_help', { topic }); setNotice('Help opened in your browser.'); }
    catch { setNotice("Couldn't open help. Open the GitHub CLI website for installation and sign-in instructions."); }
  }
  async function copy() {
    if (!error) return;
    try {
      await navigator.clipboard.writeText(['Bridge AI connection', `Operation: ${failedAction === 'remove' ? 'Removing sign-in' : 'Connecting AI'}`, `Category: ${error.category}`,
        ...error.codes.map(code => `OS code: ${code}`)].join('\n'));
      setNotice('Details copied. Private diagnostic text is not included.');
    } catch { setNotice("Couldn't copy. Share the visible error message for help."); }
  }
  function choose() {
    onConnectRequest?.();
    refocus.current = document.activeElement === retry.current ? 'connect' : undefined;
    if (enabled) void perform('connect'); else if (queueable && !paused) setQueued(true);
  }
  const removalFailed = !!error && failedAction === 'remove';
  const cliUnavailable = !removalFailed && error?.category === 'modelCaptureUnavailable';
  const signInCheck = !removalFailed && error && ['modelCaptureFailed', 'modelCaptureInvalid', 'modelCaptureTimedOut'].includes(error.category);
  return <section ref={root} className="card setup-model" aria-labelledby="model-title" aria-busy={!!busy}>
    <h2 id="model-title">Connect the AI</h2>
    {!ready && <><p className="muted">Use GitHub Copilot. GitHub CLI must be installed and signed in to github.com.</p>
    {!enabled && (queueable || queued) ? <p className="muted">Choose now to connect automatically when the local cluster is ready.</p>
      : !enabled ? <p className="muted">Start the local cluster before connecting the AI.</p>
      : <p className="muted">Your sign-in is shared only with the local cluster.</p>}</>}
    <div className={error ? 'workspace-failure' : undefined} role={error ? 'alert' : undefined}>
      <div className={error ? 'workspace-failure-explanation' : undefined}>
        {error && <><p className="failure-summary">{removalFailed && error.category === 'unknown' ? 'Bridge could not confirm sign-in removal.' : error.message}</p>
        <p>{failedAction === 'remove' ? 'Press Retry removal to retry cleanup. If it happens again, copy the details for help.'
          : cliUnavailable ? 'Install GitHub CLI if needed, then sign in to github.com in your terminal:'
          : signInCheck ? 'Check your GitHub CLI sign-in in your terminal:'
          : error.category === 'modelBusy' ? 'Wait for the other operation to finish, then press Check again.'
          : 'Press Check again to reconnect. If it happens again, copy the details for help.'}</p>
        {(cliUnavailable || signInCheck) && <pre>{cliUnavailable ? 'gh auth login --hostname github.com --web' : 'gh auth status --hostname github.com'}</pre>}
        <p>The local cluster and your agent files are kept. {failedAction === 'remove' ? 'Removal has not been confirmed.' : 'A sign-in may already have been saved.'}</p>
        {failedAction === 'connect' && <p className="muted">Check again reconnects the AI using your saved sign-in when available.</p>}</>}
        <div className="workspace-step-actions">
          {confirmRemoval ? <div role="group" aria-label="Remove sign-in confirmation">
            <p>Disconnect the AI and remove Bridge’s saved sign-in? GitHub CLI stays signed in.</p>
            <div className="workspace-step-actions">
              <Button ref={cancelRemoval} type="button" kind="primary" onClick={() => {
                refocus.current = document.activeElement === cancelRemoval.current ? 'remove' : undefined;
                setConfirmRemoval(false);
              }}>Cancel</Button>
              <Button ref={removal} type="button" kind="secondary" className="button-danger" disabled={!enabled || paused || !!busy} onClick={() => {
                refocus.current = document.activeElement === removal.current ? 'remove' : undefined;
                void perform('remove');
              }}>Remove sign-in</Button>
            </div>
          </div> : <>
          {cliUnavailable && <Button type="button" kind="secondary" onClick={() => { void help('githubCliInstall'); }}>Install GitHub CLI</Button>}
          {(cliUnavailable || signInCheck) && <Button type="button" kind="secondary" onClick={() => { void help('githubSignIn'); }}>Sign-in help</Button>}
          {queued || busy || ready ? <div ref={status} role={ready ? 'status' : undefined} tabIndex={-1} className="model-state">
            <StatusPill state={ready ? 'passed' : queued ? 'not-started' : 'running'}>{busy === 'remove' ? 'Removing sign-in…' : ready ? 'Connected' : queued ? 'Queued' : 'Connecting the AI…'}</StatusPill>
            {queued && <><span className="muted">Connects when the local cluster is ready.</span>
              <Button type="button" kind="quiet" onClick={() => setQueued(false)}>Cancel queued connection</Button></>}
          </div>
          : !removalFailed && <Button key="model-connect" ref={retry} type="button" kind={primary ? 'primary' : 'secondary'} disabled={(!enabled && !queueable) || paused || !!busy} onClick={choose}>
            {error && failedAction === 'connect' ? 'Check again' : 'Connect the AI'}
          </Button>}
          {hasSignIn && busy !== 'remove' && <Button ref={removal} type="button" kind={removalFailed ? 'primary' : 'quiet'} className={removalFailed ? undefined : 'button-danger'} disabled={!enabled || paused || !!busy}
            onClick={() => {
              refocus.current = document.activeElement === removal.current ? 'remove' : undefined;
              if (removalFailed) void perform('remove'); else setConfirmRemoval(true);
            }}>{removalFailed ? 'Retry removal' : 'Remove sign-in'}</Button>}
          {error && <Button type="button" kind="secondary" onClick={() => { void copy(); }}>Copy details for help</Button>}
          </>}
        </div>
      </div>
    </div>
    {busy === 'connect' && <p role="status">Connecting with your saved sign-in. This may take a few seconds…</p>}
    {busy === 'remove' && <p role="status">Disconnecting the AI and removing Bridge’s saved sign-in…</p>}
    {removed && <p role="status">Bridge no longer has a saved sign-in. GitHub CLI stays signed in.</p>}
    {notice && <p role="status">{notice}</p>}
  </section>;
}
