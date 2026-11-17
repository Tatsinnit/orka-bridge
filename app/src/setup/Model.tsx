import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Button } from '../components/Button';
import { StatusPill } from '../components/DesignSystem';
import { Disclosure } from '../components/Disclosure';
import { companies, connectKey, connectModel, copilotModel, type KeyVendor, keyFailure, type ModelConnection, type ModelFailure, type ModelRecovery, removeModel, resetModel, validKey } from './modelState';
import type { HelpTopic } from './recovery-info';

export type ModelState = 'idle' | 'queued' | 'connecting' | 'removing' | 'resetting' | 'resetFailed' | 'removalFailed' | 'connected' | 'failed';
// `enabled` is native permission to touch credentials. `queueable` only permits remembering the
// creator's choice while the workspace is still arriving. `away` means another place was chosen.
// Each increase of changeRequest explicitly asks to change the AI provider.
export default function Model({ enabled, queueable = false, paused = false, away = false, primary = true, changeRequest = 0, recoveryTarget, choiceTarget, inline = false, onPlaceCard = false, onState, onSettled, onConnectRequest }: {
  choiceTarget?: HTMLElement | null; inline?: boolean;
  enabled: boolean; queueable?: boolean; paused?: boolean; away?: boolean; primary?: boolean; changeRequest?: number; recoveryTarget?: HTMLElement | null; onPlaceCard?: boolean; onState?: (state: ModelState) => void; onSettled?: () => void; onConnectRequest?: () => void;
}) {
  // In-memory consent only. It never carries a credential and is consumed once.
  const [queued, setQueued] = useState(false);
  const [choice, setChoice] = useState<'copilot' | 'key'>('copilot');
  const [vendor, setVendor] = useState<KeyVendor>('anthropic');
  const [connection, setConnection] = useState<ModelConnection>();
  const keyField = useRef<HTMLInputElement>(null);
  const [hasKey, setHasKey] = useState(false);
  const clearKey = useCallback(() => { if (keyField.current) keyField.current.value = ''; setHasKey(false); }, []);
  const [expanded, setExpanded] = useState(false);
  const [changing, setChanging] = useState(false);
  const changeButton = useRef<HTMLButtonElement>(null);
  function closeChange() { clearKey(); setExpanded(false); setChanging(false); setConfirmRemoval(false); changeFocus.current = changing || compact ? 'change' : 'panel'; }
  const company = companies[vendor];
  const root = useRef<HTMLElement>(null);
  const status = useRef<HTMLDivElement>(null);
  const focusWithin = useRef(false);
  useEffect(() => {
    // A portal move replaces DOM, not consent. Restore focus only when that DOM owned it.
    if (focusWithin.current && document.activeElement === document.body && !root.current?.closest('[hidden]')) {
      refocus.current = 'connect';
      (status.current ?? heading.current)?.focus();
    }
  }, [inline, choiceTarget]);
  const retry = useRef<HTMLButtonElement>(null);
  const removal = useRef<HTMLButtonElement>(null);
  const cancelRemoval = useRef<HTMLButtonElement>(null);
  const [confirmRemoval, setConfirmRemoval] = useState(false);
  const heading = useRef<HTMLHeadingElement>(null);
  // An explicit change moves focus once rendered: to the confirmation, else the connection choice.
  const changeFocus = useRef<'confirm' | 'choice' | 'panel' | 'change'>(undefined);
  const handledChange = useRef(changeRequest);
  // Focus follows the keyboard user's operation, then its retry or next available action.
  const refocus = useRef<'connect' | 'remove'>(undefined);
  const [busy, setBusy] = useState<'connect' | 'remove' | 'reset'>();
  const [recovery, setRecovery] = useState<ModelRecovery>();
  const [confirmReset, setConfirmReset] = useState(false);
  const [anotherProvider, setAnotherProvider] = useState(false);
  const [resetDone, setResetDone] = useState(false);
  const [cardRecovery, setCardRecovery] = useState(false);
  const [resetAttempts, setResetAttempts] = useState(0);
  const [details, setDetails] = useState(false);
  const [removed, setRemoved] = useState(false);
  const [connected, setConnected] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const ready = enabled && connected;
  const hasSignIn = !removed && attempted;
  const [error, setError] = useState<ModelFailure>();
  const [notice, setNotice] = useState('');
  const [failedAction, setFailedAction] = useState<'connect' | 'remove' | 'reset'>('connect');
  const removalFailed = !!error && (failedAction === 'remove' || error.category === 'modelKeyCleanup');
  const actionReason = paused ? 'Wait for the workspace action to finish.' : busy ? `Wait for the AI ${busy === 'connect' ? 'connection' : busy === 'remove' ? 'removal' : 'reset'} to finish.` : !enabled ? 'Start the workspace before changing the AI connection.' : undefined;
  const providerReason = busy ? actionReason : hasSignIn ? 'Remove the saved sign-in or key before choosing another provider.' : undefined;
  // Clearing an error advances the same card flow; it must not hide that flow's next state.
  useEffect(() => {
    if (!onPlaceCard) setCardRecovery(false);
    else if (error) setCardRecovery(true);
  }, [onPlaceCard, error]);
  const epoch = useRef(0);
  const active = useRef(false);
  useEffect(() => {
    epoch.current++;
    active.current = false;
    if (!enabled) { setBusy(undefined); setConnected(false); setRemoved(false); setError(undefined); setRecovery(undefined); setConfirmReset(false); }
    return () => { epoch.current++; };
  }, [enabled]);
  useEffect(() => {
    onState?.(!enabled ? queued ? 'queued' : 'idle' : busy === 'connect' ? 'connecting' : busy === 'remove' ? 'removing' : busy === 'reset' ? 'resetting' : ready ? 'connected' : error ? failedAction === 'reset' ? 'resetFailed' : failedAction === 'remove' || error.category === 'modelKeyCleanup' ? 'removalFailed' : 'failed' : queued ? 'queued' : 'idle');
  }, [enabled, busy, ready, error, queued, failedAction, onState]);
  useEffect(() => { if (away || !enabled) clearKey(); if (away) setQueued(false); }, [away, enabled, clearKey]);
  useEffect(() => {
    // Setup can stay mounted under a hidden tab or cluster page.
    const observer = new MutationObserver(() => { if (root.current?.closest('[hidden]')) clearKey(); });
    observer.observe(document.body, { subtree: true, attributes: true, attributeFilter: ['hidden'] });
    return () => observer.disconnect();
  }, [clearKey]);
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
    if (confirmRemoval || confirmReset) { cancelRemoval.current?.focus(); return; }
    const next = refocus.current === 'remove' ? removal.current ?? retry.current : retry.current;
    if (next?.disabled) keyField.current?.focus(); else next?.focus();
    if (document.activeElement === document.body) status.current?.focus();
    refocus.current = undefined;
  }, [busy, queued, ready, error, confirmRemoval, confirmReset, recoveryTarget]);
  // Never replaces a saved sign-in directly: the creator confirms removal first, then chooses again.
  useEffect(() => {
    if (changeRequest === handledChange.current) return;
    handledChange.current = changeRequest;
    if (changeRequest <= 0) return;
    if (recovery && enabled && !paused && !busy) {
      setAnotherProvider(true); setConfirmReset(true); refocus.current = 'connect';
      return;
    }
    const confirm = hasSignIn && enabled && !paused && !busy && !removalFailed;
    changeFocus.current = confirm ? 'confirm' : 'choice';
    if (confirm) { setChanging(true); changeFocus.current = 'panel'; }
    else setExpanded(true);
  }, [changeRequest, hasSignIn, enabled, paused, busy, removalFailed, recovery]);
  useEffect(() => {
    // A confirmation focuses once it renders; a hidden card never takes focus.
    if (!changeFocus.current || (changeFocus.current === 'confirm' && !confirmRemoval)) return;
    if (changeFocus.current === 'panel' && !removal.current || changeFocus.current === 'change' && !changeButton.current) return;
    const intent = changeFocus.current;
    changeFocus.current = undefined;
    if (root.current?.closest('[hidden]')) return;
    const target = intent === 'change' ? changeButton.current : intent === 'panel' ? removal.current : confirmRemoval ? cancelRemoval.current
      : busy ? heading.current
      : changing || removalFailed ? removal.current
      : root.current?.querySelector<HTMLElement>('.model-choice:not(:disabled) input:checked');
    (target ?? heading.current)?.focus();
  });
  async function perform(action: 'connect' | 'remove' | 'reset') {
    if (!enabled || paused || active.current || (action === 'connect' && ready)) return;
    if (action === 'reset' && !recovery) return;
    const key = action === 'connect' && choice === 'key' ? keyField.current?.value ?? '' : '';
    if (action === 'connect' && choice === 'key' && (key || !hasSignIn) && !validKey(vendor, key)) {
      clearKey(); setFailedAction('connect'); setError({ category: 'modelKeyInvalid', codes: [], message: 'Paste the whole key.' }); return;
    }
    clearKey();
    active.current = true;
    const request = ++epoch.current;
    const before = { attempted, removed };
    setBusy(action); setError(undefined); setRemoved(false); setNotice(''); setFailedAction(action);
    setAttempted(true); setConfirmRemoval(false); setConfirmReset(false); setResetDone(false);
    if (action === 'remove') setChanging(false);
    if (action === 'reset') setResetAttempts(value => value + 1);
    if (action !== 'connect') setConnected(false);
    const result = await (action === 'reset' ? resetModel() : action === 'remove' ? removeModel() : choice === 'key' && key ? connectKey(vendor, key) : connectModel(choice === 'key' ? vendor : undefined));
    if (request !== epoch.current) return;
    active.current = false; setBusy(undefined);
    if (result.ok) {
      setConnected(action === 'connect'); setRemoved(action !== 'connect'); setChanging(false);
      if (action === 'reset') { setResetDone(true); setRecovery(undefined); setAttempted(false); setDetails(false); if (anotherProvider) setChoice('key'); }
      if (action === 'connect') {
        const info = result.connection ?? { kind: 'copilot', model: copilotModel };
        setConnection(info); setChoice(info.kind);
        if (info.kind === 'key') setVendor(info.vendor);
      } else setConnection(undefined);
    } else {
      if (result.vendor) { setChoice('key'); setVendor(result.vendor); }
      if (action === 'connect' && result.category === 'modelBusy') { setAttempted(before.attempted); setRemoved(before.removed); }
      if (['modelCaptureUnavailable', 'modelCaptureFailed', 'modelCaptureInvalid', 'modelCaptureTimedOut'].includes(result.category)) setAttempted(false);
      setError(result);
      if (result.recovery && (action === 'reset' || result.category === 'modelInstallationMismatch')) setRecovery(result.recovery);
      else if (action === 'reset') setRecovery(value => value && { ...value, resetStage: undefined });
      if ((choice === 'key' || result.vendor) && ['modelCredentialRejected', 'modelProviderUnreachable', 'modelModelUnavailable'].includes(result.category)) setRemoved(true);
    }
    onSettled?.();
  }
  async function help(topic: HelpTopic) {
    try { await invoke('setup_open_help', { topic }); setNotice('Help opened in your browser.'); }
    catch { setNotice(choice === 'key' ? `Couldn't open help. Open ${company.site} to create a key.` : "Couldn't open help. Open the GitHub CLI website for installation and sign-in instructions."); }
  }
  async function copy() {
    if (!error) return;
    try {
      await navigator.clipboard.writeText(['Bridge AI connection', `Operation: ${failedAction === 'reset' ? 'Resetting AI connection' : failedAction === 'remove' ? 'Removing sign-in' : 'Connecting AI'}`, `Category: ${error.category}`,
        ...(error.cleanup ? [`Cleanup operation: ${error.cleanup[0]}`, `Cleanup class: ${error.cleanup[1]}`] : []),
        ...error.codes.map(code => `OS code: ${code}`),
        ...(recovery ? [`Installation: ${recovery.installation}`, `Saved installation: ${recovery.savedInstallation}`, `Native credential slot: ${recovery.credentialSlot}`, `Reset stage: ${recovery.resetStage ?? (failedAction === 'reset' ? 'Unconfirmed' : 'Not started')}`] : [])].join('\n'));
      setNotice('Details copied. No keys or sign-in tokens are included.');
    } catch { setNotice("Couldn't copy. Share the visible error message for help."); }
  }
  function choose() {
    onConnectRequest?.();
    refocus.current = document.activeElement === retry.current ? 'connect' : undefined;
    if (enabled) void perform('connect'); else if (choice === 'copilot' && queueable && !paused) { setQueued(true); setExpanded(false); changeButton.current?.focus(); }
  }
  const cliUnavailable = !removalFailed && error?.category === 'modelCaptureUnavailable';
  const signInCheck = !removalFailed && error && ['modelCaptureFailed', 'modelCaptureInvalid', 'modelCaptureTimedOut'].includes(error.category);
  function chooseKind(kind: 'copilot' | 'key') { clearKey(); setQueued(false); setChoice(kind); setError(undefined); }
  const keyMode = choice === 'key';
  const keyProblem = keyMode && error && !removalFailed;
  const removalAction = hasSignIn && !busy && <Button ref={removal} type="button" kind={removalFailed ? 'primary' : 'quiet'} className={removalFailed ? undefined : 'button-danger'} disabled={!enabled || paused || !!busy}
            onClick={() => {
              refocus.current = document.activeElement === removal.current ? 'remove' : undefined;
              if (removalFailed) void perform('remove'); else setConfirmRemoval(true);
            }}>{removalFailed ? 'Retry removal' : changing ? keyMode ? 'Remove saved key' : 'Remove saved sign-in' : keyMode ? 'Remove key' : 'Remove sign-in'}</Button>;
  if (recovery) {
    const resetFailed = failedAction === 'reset' && !!error;
    const custody = recovery.resetStage === 'recordRemovalUnconfirmed'
      ? 'Native credential removed; saved record removal not confirmed.'
      : recovery.resetStage === 'credentialRemovalUnconfirmed' ? 'Native credential removal not confirmed; saved record kept.' : resetFailed ? 'Bridge could not check what is still saved. The identifiers below are from the last confirmed record.' : 'Saved record: model-v1. Native credential removal has not been requested.';
    const content = <section ref={root} className={recoveryTarget ? 'model-recovery' : 'card setup-model model-recovery'} aria-label="Saved AI connection" aria-busy={!!busy}>
      {actionReason && <p className="muted">{actionReason}</p>}
      {busy === 'reset' ? <div ref={status} role="status" tabIndex={-1} className="model-state"><StatusPill state="running">Resetting</StatusPill><p>Removing Bridge's saved AI connection. Your workspace and agents aren't touched.</p></div>
        : <div className="workspace-failure" role="alert"><div className="workspace-failure-explanation">
          {resetFailed ? <><h2 className="failure-summary">Bridge couldn't reset the AI connection</h2><p>{resetAttempts > 1 ? 'Reset failed again. Copy the details for whoever helps you with Bridge; Show details lists what Bridge could confirm.' : "Bridge couldn't confirm the saved connection was removed. Try once more. If it fails again, copy the details for whoever helps you with Bridge."}</p></>
            : <><p className="failure-summary">Bridge's saved AI connection is from an earlier setup.</p><p>This happens after a reinstall. Reset it and choose the AI again. Your agents are safe.</p></>}
          {confirmReset ? <div role="group" aria-label="Reset AI connection confirmation"><h3>Reset the AI connection?</h3><p>Bridge forgets its saved AI connection for this workspace. Your GitHub account and your agents don't change. Use another provider starts with this same reset.</p><div className="workspace-step-actions">
            <Button ref={retry} kind="primary" disabled={!enabled || paused || !!busy} onClick={() => { refocus.current = 'connect'; void perform('reset'); }}>Reset</Button>
            <Button ref={cancelRemoval} kind="secondary" onClick={() => { refocus.current = 'connect'; setConfirmReset(false); }}>Cancel</Button>
          </div></div> : <div className="workspace-step-actions">
            <Button ref={retry} kind="primary" disabled={!enabled || paused || !!busy} onClick={() => { refocus.current = 'connect'; if (resetFailed) void perform('reset'); else { setAnotherProvider(false); setConfirmReset(true); } }}>{resetFailed ? 'Reset again' : 'Reset AI connection'}</Button>
            {!resetFailed && <Button kind="secondary" disabled={!enabled || paused || !!busy} onClick={() => { refocus.current = 'connect'; setAnotherProvider(true); setConfirmReset(true); }}>Use another provider</Button>}
            <Button kind="secondary" onClick={() => { void copy(); }}>Copy details for help</Button>
          </div>}
          <Disclosure label={details ? 'Hide details' : 'Show details'} open={details} onOpenChange={setDetails} controls="model-recovery-details">
            <div id="model-recovery-details" hidden={!details}><p>{custody}</p><dl className="model-recovery-details"><dt>Category</dt><dd>{error?.category ?? 'modelInstallationMismatch'}</dd><dt>This installation</dt><dd>{recovery.installation}</dd><dt>Saved installation</dt><dd>{recovery.savedInstallation}</dd><dt>Saved record</dt><dd>Bridge app data: model-v1</dd><dt>Native credential slot</dt><dd>{recovery.credentialSlot}</dd></dl></div>
          </Disclosure>
        </div></div>}
      {notice && <p role="status">{notice}</p>}
    </section>;
    return recoveryTarget ? createPortal(content, recoveryTarget) : content;
  }
  const portaled = !!recoveryTarget && (!!error || onPlaceCard && (resetDone || cardRecovery));
  const compact = inline && !portaled;
  const content = <section ref={root} className={portaled || compact ? 'setup-model' : 'card setup-model'} aria-labelledby="model-title" aria-busy={!!busy} onFocusCapture={() => { focusWithin.current = true; }} onBlurCapture={event => { if (event.relatedTarget) focusWithin.current = !!root.current?.contains(event.relatedTarget); }} onKeyDown={event => { if (event.key === 'Escape' && !busy && (changing || confirmRemoval || compact && expanded)) { event.stopPropagation(); closeChange(); } }}>
    <div className={compact ? 'model-inline-head' : 'steps-head'}>
      <h2 id="model-title" ref={heading} tabIndex={-1} className={compact ? 'sr-only' : undefined}>{removalFailed ? `Couldn't remove the saved ${keyMode ? 'key' : 'sign-in'}` : 'Connect the AI'}</h2>
      {compact && <span role="status" aria-atomic="true">{keyMode ? 'Your own API key' : 'GitHub Copilot'}{queued ? ', connects when the workspace is ready' : ', connect at step 3'}</span>}
      {ready && <div ref={status} role="status" tabIndex={-1} className="model-state"><StatusPill state="passed">Connected</StatusPill></div>}
      {compact && <Button ref={changeButton} type="button" kind="quiet" disabled={paused || !!busy} aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>Change</Button>}
    </div>
    <div hidden={compact && !expanded}>
    {actionReason && (paused || busy || removalFailed) && <p className="muted">{actionReason}</p>}
    {!ready && busy !== 'remove' && !removalFailed && <>
      <fieldset className="model-choice" title={providerReason} disabled={!!busy || hasSignIn}>
        <legend className="sr-only">AI connection</legend>
        <label><input type="radio" name="model-kind" checked={!keyMode} onChange={() => chooseKind('copilot')} /> GitHub Copilot</label>
        <label><input type="radio" name="model-kind" checked={keyMode} onChange={() => chooseKind('key')} /> Your own API key <span className="muted">(OpenAI, Anthropic)</span></label>
      </fieldset>
      {providerReason && providerReason !== actionReason && <p className="muted">{providerReason}</p>}
      {keyMode ? <div className="model-key-form">
        <fieldset className="view-switch"><legend className="sr-only">Company</legend>
          {(['openai', 'anthropic'] as const).map(item => <button key={item} type="button" aria-pressed={vendor === item} disabled={!!busy || hasSignIn} onClick={() => { clearKey(); setVendor(item); setError(undefined); }}>{companies[item].name}</button>)}
        </fieldset>
        <label htmlFor="model-api-key">API key</label>
        <input ref={keyField} onChange={event => setHasKey(!!event.currentTarget.value.trim())} id="model-api-key" type="password" autoComplete="off" spellCheck={false} autoCapitalize="none" disabled={!enabled || paused || !!busy} aria-describedby="model-key-help" aria-invalid={error?.category === 'modelKeyInvalid' || error?.category === 'modelCredentialRejected'} />
        <p id="model-key-help" className="muted">Stored on this computer, never in your agent files.</p>
        <a href={`https://${company.site}`} onClick={event => { event.preventDefault(); void help(vendor === 'openai' ? 'openaiKey' : 'anthropicKey'); }}>Where do I get a key?</a>
      </div> : <p className="muted">Use GitHub Copilot. GitHub CLI must be installed and signed in to github.com.</p>}
      <p>Model <code>{keyMode ? company.model : copilotModel}</code>, a good default.</p>
      {!enabled && keyMode ? <p className="muted">Bridge can't queue a key. Enter it once the workspace is ready.</p>
        : !enabled && (queueable || queued) ? <p className="muted">Choose now to connect automatically when the workspace is ready.</p>
        : !enabled ? <p className="muted">Start the workspace before connecting the AI.</p>
        : !keyMode && <p className="muted">Your sign-in is shared only with your workspace.</p>}
    </>}
    {ready && connection && <>
      <p>{connection.kind === 'key' ? `${companies[connection.vendor].name} with your API key` : 'GitHub Copilot'} · <code>{connection.model}</code></p>
      {connection.kind === 'copilot' && <p>Saved GitHub sign-in: {connection.account ?? 'account unavailable'}</p>}
    </>}
    <div className={error ? 'workspace-failure' : undefined} role={error ? 'alert' : undefined}>
      <div className={error ? 'workspace-failure-explanation' : undefined}>
        {error && <><p className="failure-summary">{removalFailed && error.category === 'unknown' ? `Bridge could not confirm ${keyMode ? 'key' : 'sign-in'} removal.` : keyMode ? keyFailure(error, vendor) : error.message}</p>
        {!keyProblem && <><p>{removalFailed ? 'Press Retry removal to retry cleanup. If it happens again, copy the details for help.'
          : cliUnavailable ? 'Install GitHub CLI if needed, then sign in to github.com in your terminal:'
          : signInCheck ? 'Check your GitHub CLI sign-in in your terminal:'
          : error.category === 'modelBusy' ? 'Wait for the other operation to finish, then press Check again.'
          : 'Press Check again to reconnect. If it happens again, copy the details for help.'}</p>
        {(cliUnavailable || signInCheck) && <pre>{cliUnavailable ? 'gh auth login --hostname github.com --web' : 'gh auth status --hostname github.com'}</pre>}
        <p>Your workspace and agent files are kept. {removalFailed ? 'Removal has not been confirmed.' : 'A sign-in may already have been saved.'}</p>
        {!removalFailed && <p className="muted">Check again reconnects the AI using your saved sign-in when available.</p>}</>}
        </>}
        <div className="workspace-step-actions">
          {changing && !confirmRemoval && <fieldset className="workspace-change" aria-label="Change the AI">
            <legend>Change the AI</legend><p>Remove the saved {keyMode ? 'key' : 'sign-in'} before choosing another provider.</p>
            <div className="workspace-step-actions">{removalAction}<Button type="button" kind="quiet" onClick={closeChange}>Done</Button></div>
          </fieldset>}
          {confirmRemoval ? <div role="group" aria-label={`Remove ${keyMode ? 'key' : 'sign-in'} confirmation`}>
            <p>{keyMode ? 'Disconnect the AI and remove Bridge’s saved key? This does not revoke the key with the company.' : 'Disconnect the AI and remove Bridge’s saved sign-in? GitHub CLI stays signed in.'}</p>
            <div className="workspace-step-actions">
              <Button ref={cancelRemoval} type="button" kind="primary" onClick={() => {
                refocus.current = document.activeElement === cancelRemoval.current ? 'remove' : undefined;
                setConfirmRemoval(false);
              }}>Cancel</Button>
              <Button ref={removal} type="button" kind="secondary" className="button-danger" disabled={!enabled || paused || !!busy} onClick={() => {
                refocus.current = document.activeElement === removal.current ? 'remove' : undefined;
                void perform('remove');
              }}>{keyMode ? 'Remove key' : 'Remove sign-in'}</Button>
            </div>
          </div> : <>
          {queued || busy ? <div ref={status} role={queued ? 'status' : undefined} tabIndex={-1} className="model-state">
            <StatusPill state={ready ? 'passed' : queued ? 'not-started' : 'running'}>{busy === 'remove' ? 'Removing' : ready ? 'Connected' : queued ? 'Queued' : keyMode ? 'Checking' : 'Connecting'}</StatusPill>
            {busy && (busy === 'remove' || !keyMode) && <span className="status-description">{busy === 'remove' ? keyMode ? 'Removing key…' : 'Removing sign-in…' : 'Connecting the AI…'}</span>}
            {queued && <><span className="status-description">Connects when the workspace is ready.</span>{(!compact || expanded) && <Button type="button" kind="secondary" aria-label={compact ? undefined : 'Cancel queued connection'} onClick={() => { setQueued(false); if (compact) { setExpanded(false); changeButton.current?.focus(); } }}>{compact ? "Don't connect automatically" : 'Cancel'}</Button>}</>}
          </div>
          : !ready && !removalFailed && <Button key="model-connect" ref={retry} type="button" kind={primary ? 'primary' : 'secondary'} disabled={(!enabled && (!queueable || keyMode)) || paused || !!busy || (keyMode && !hasKey && !hasSignIn)} onClick={choose}>
            {error && failedAction === 'connect' ? 'Check again' : 'Connect the AI'}
          </Button>}
          {ready && !changing && <Button ref={changeButton} type="button" kind="quiet" title={actionReason} disabled={paused || !!busy} aria-expanded={false} onClick={event => { if (document.activeElement === event.currentTarget) changeFocus.current = 'panel'; setChanging(true); }}>Change</Button>}
          {removalFailed && removalAction}
          {error && <Button type="button" kind="secondary" onClick={() => { void copy(); }}>Copy details for help</Button>}
          {cliUnavailable && <Button type="button" kind="quiet" onClick={() => { void help('githubCliInstall'); }}>Install GitHub CLI</Button>}
          {(cliUnavailable || signInCheck) && <Button type="button" kind="quiet" onClick={() => { void help('githubSignIn'); }}>Sign-in help</Button>}
          {!ready && !removalFailed && removalAction}
          </>}
        </div>
      </div>
    </div>
    {busy === 'connect' && <p role="status">{keyMode ? `Asking ${company.name} to confirm the key…` : 'Connecting with your saved sign-in. This may take a few seconds…'}</p>}
    {busy === 'remove' && <p role="status">Disconnecting the AI and removing Bridge’s saved {keyMode ? 'key' : 'sign-in'}…</p>}
    {resetDone && <p role="status">Connection reset. Choose the AI again.</p>}
    {removed && !resetDone && <p role="status">{keyMode ? 'Key removed.' : 'Saved sign-in removed.'}</p>}
    {notice && <p role="status">{notice}</p>}
    {compact && expanded && <Button type="button" kind="quiet" onClick={closeChange}>Done</Button>}
    </div>
  </section>;
  return portaled && recoveryTarget ? createPortal(content, recoveryTarget) : compact && choiceTarget ? createPortal(content, choiceTarget) : content;
}
