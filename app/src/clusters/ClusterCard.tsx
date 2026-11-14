import { useEffect, useRef, useState } from 'react';
import { Button } from '../components/Button';
import { StatusIcon } from '../components/DesignSystem';
import { type AccessView, type ClusterFailure, type ContextView, connectCluster, contexts, discover, failedCheck, reloadContexts, type SelectedPlace, type TargetView } from './api';
import { clusterDetails, clusterRecovery } from './recovery';
import './clusters.css';

type Failure = { failure: ClusterFailure; operation: 'Read kubeconfig' | 'Reload kubeconfig' | 'Discover Orka' | 'Use cluster' };
type Entry = { revision: number; current?: SelectedPlace; intent?: string };
export default function ClusterCard({ active = true, entry, selection, onPlace, onOpenAgent }: { active?: boolean; entry?: Entry; selection?: SelectedPlace; onPlace: (place: SelectedPlace) => void; onOpenAgent: () => void }) {
  const [choices, setChoices] = useState<ContextView[]>([]);
  const [contextId, setContextId] = useState('');
  const [targets, setTargets] = useState<TargetView[]>();
  const [targetId, setTargetId] = useState('');
  const [access, setAccess] = useState<AccessView>();
  const [providerId, setProviderId] = useState('');
  const [account, setAccount] = useState('');
  const [needsAccount, setNeedsAccount] = useState(false);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<Failure>();
  const [reloading, setReloading] = useState(false);
  const [notice, setNotice] = useState('');
  const [details, setDetails] = useState(false);
  const [changingContext, setChangingContext] = useState(false);
  const epoch = useRef(0); const admitted = useRef(false); const reloadActive = useRef(false);
  const reloadInterrupted = useRef(false); const initialLoaded = useRef(false); const wasActive = useRef(active);
  const providerName = useRef('');
  const accountField = useRef<HTMLInputElement>(null);
  const providerField = useRef<HTMLSelectElement>(null);
  function readContexts() {
    const request = ++epoch.current; setBusy(true); setError(undefined);
    void contexts().then(v => { if (request === epoch.current) { setChoices(v); initialLoaded.current = true; } }, error => {
      if (request === epoch.current) { setError({ failure: failedCheck(error, 'configuration'), operation: 'Read kubeconfig' }); initialLoaded.current = false; }
    }).finally(() => { if (request === epoch.current) setBusy(false); });
  }
  useEffect(() => {
    if (!entry) readContexts();
    return () => { epoch.current++; };
  }, []);
  useEffect(() => {
    if (!entry) return;
    // Entry intent is presentation only. Only Manage receives App's still-current capability.
    // Reload clears the choices; a new entry must replace an interrupted reload's fenced reply.
    if (reloadActive.current || reloadInterrupted.current) initialLoaded.current = false;
    epoch.current++; admitted.current = false; reloadActive.current = false; reloadInterrupted.current = false;
    const current = entry.current?.kind === 'cluster' ? entry.current : undefined;
    const target = current?.target;
    setContextId(target?.context_id ?? ''); setTargets(target ? [target] : undefined); setTargetId(target?.id ?? '');
    setAccess(target ? { status: 'connected', providers: current?.providers ?? [], failure: null } : undefined);
    setProviderId(current?.providerId ?? ''); providerName.current = current?.providers?.find(p => p.id === current.providerId)?.name ?? '';
    // A Manage visit may retain the name already entered for this exact target handle.
    if (!target || target.id !== targetId) { setAccount(''); setNeedsAccount(false); }
    setError(undefined); setDetails(false); setNotice(''); setChangingContext(false);
    setReloading(false);
    // An entry reset also fences a pending local kubeconfig read; restart just that read.
    if (!initialLoaded.current) readContexts(); else setBusy(false);
  }, [entry]);
  useEffect(() => {
    if (!active) return;
    if (access?.status === 'needServiceAccount') accountField.current?.focus();
    if (access?.status === 'connected') providerField.current?.focus();
  }, [access, active]);
  useEffect(() => {
    const was = wasActive.current; wasActive.current = active;
    if (!active) {
      // Retain configuration, not active confirmation; late replies cannot reselect it.
      if (reloadActive.current) reloadInterrupted.current = true;
      epoch.current++; admitted.current = false; reloadActive.current = false;
      setBusy(false); setReloading(false);
      // A completed Setup subview survives tab navigation. Standalone callers keep the
      // original fresh-Use-on-reactivation behavior; pending replies are always fenced.
      if (!entry) setAccess(undefined);
      return;
    }
    // Resume a configuration read interrupted by deactivation; a completed read (success,
    // failure, or one already loaded) is left alone, and late replies to the interrupted
    // request stay discarded by the epoch check above.
    if (!was && reloadInterrupted.current) { reloadInterrupted.current = false; void reload(); }
    else if (!was && !initialLoaded.current) readContexts();
  }, [active]);
  useEffect(() => {
    // Opening a local document elsewhere also revokes this subview's confirmation.
    if (selection && (selection.kind !== 'cluster' || !selection.target)) setAccess(undefined);
  }, [selection]);
  const target = targets?.find(t => t.id === targetId);
  const recovery = error && clusterRecovery(error.failure);
  async function reload() {
    if (!active || reloadActive.current) return;
    reloadActive.current = true; initialLoaded.current = false; admitted.current = false; const request = ++epoch.current;
    setReloading(true); setBusy(true); setChoices([]); setContextId(''); setTargets(undefined); setTargetId('');
    setAccess(undefined); setProviderId(''); providerName.current = ''; setAccount(''); setNeedsAccount(false); setError(undefined); setDetails(false); setNotice('');
    onPlace({ kind: 'cluster' });
    try { const value = await reloadContexts(); if (request === epoch.current) { setChoices(value); initialLoaded.current = true; } }
    catch (error) { if (request === epoch.current) setError({ failure: failedCheck(error, 'configuration'), operation: 'Reload kubeconfig' }); }
    finally { if (request === epoch.current) { reloadActive.current = false; setReloading(false); setBusy(false); } }
  }
  async function copy() {
    if (!error) return;
    try { await navigator.clipboard.writeText(clusterDetails(error.failure, error.operation)); setNotice('Details copied. Private diagnostic text is not included.'); }
    catch { setNotice("Couldn't copy. Share the visible error message for help."); }
  }
  async function check(id: string) {
    if (!active) return;
    const request = ++epoch.current;
    setContextId(id); setTargets(undefined); setTargetId(''); setAccess(undefined); setProviderId(''); providerName.current = ''; setAccount(''); setNeedsAccount(false); setError(undefined); setDetails(false); setNotice('');
    onPlace({ kind: 'cluster' });
    if (!id) { setBusy(false); return; }
    setBusy(true);
    try {
      const found = await discover(id);
      if (request !== epoch.current) return;
      setTargets(found); setTargetId(found.length === 1 ? found[0].id : '');
    } catch (error) { if (request === epoch.current) setError({ failure: failedCheck(error, 'clusterApi'), operation: 'Discover Orka' }); }
    finally { if (request === epoch.current) setBusy(false); }
  }
  async function confirm() {
    if (!active || !target || busy || admitted.current) return;
    admitted.current = true; const request = ++epoch.current; setBusy(true); setError(undefined); setNotice('');
    setAccess(undefined);
    // Native Use invalidates the old access before it replies. Do not leave App holding it.
    onPlace({ kind: 'cluster' });
    try {
      const value = await connectCluster(target.id, needsAccount ? account.trim() : null);
      if (request !== epoch.current) return;
      setAccess(value);
      if (value.failure) setError({ failure: value.failure, operation: 'Use cluster' });
      if (value.status === 'needServiceAccount') setNeedsAccount(true);
      // Every successful connect mints fresh opaque provider IDs, so a remembered selection is
      // matched by name against the fresh reply rather than by the now-stale ID.
      const remembered = providerName.current;
      const byName = remembered ? value.providers.find(p => p.name === remembered) : undefined;
      const selectedProvider = byName ? byName.id
        : providerId && value.providers.some(p => p.id === providerId) ? providerId
        : value.providers.length === 1 ? value.providers[0].id : '';
      providerName.current = value.providers.find(p => p.id === selectedProvider)?.name ?? remembered;
      setProviderId(selectedProvider);
      onPlace(value.status === 'connected' ? { kind: 'cluster', target, providers: value.providers, providerId: selectedProvider } : { kind: 'cluster' });
    } catch (error) { if (request === epoch.current) setError({ failure: failedCheck(error, 'orkaConnection'), operation: 'Use cluster' }); }
    finally { if (request === epoch.current) { admitted.current = false; setBusy(false); } }
  }
  const connected = access?.status === 'connected';
  return <section className="cluster-card" aria-label="Connect an existing cluster" aria-busy={busy} hidden={!active}>
    {!connected || changingContext ? <>
      {entry?.intent && !contextId && <p className="cluster-intent">Connect {entry.intent}: choose its context to check it again.</p>}
      <label htmlFor="cluster-context">kubectl context</label>
      <select className="field" id="cluster-context" value={contextId} disabled={busy} onChange={event => { setChangingContext(false); void check(event.target.value); }}>
        <option value="">Choose a context</option>{choices.map(c => <option key={c.id} value={c.id}>{c.name}</option>)}
      </select>
      <p className="muted">Choose the context for the cluster you want to use.</p>
    </> : <Button type="button" kind="quiet" onClick={() => setChangingContext(value => !value)}>Change context</Button>}
    {!busy && !error && !choices.length && !connected && <p>No kubectl contexts were found. Add your existing context to kubeconfig, then press Reload kubeconfig.</p>}
    {busy && <p role="status">{reloading ? 'Reload pending. Waiting for any current cluster check, then reading kubeconfig…' : !choices.length ? 'Reading kubectl contexts…' : targets ? 'Confirming access…' : 'Checking the cluster…'}</p>}
    {!busy && targets?.length === 0 && <p role="status">No cluster with Orka was found using this context.</p>}
    {!connected && targets && targets.length > 1 && <><label htmlFor="cluster-installation">Orka installation</label>
      <select className="field" id="cluster-installation" disabled={busy} value={targetId} onChange={e => { setTargetId(e.target.value); setAccess(undefined); setProviderId(''); providerName.current = ''; setAccount(''); setNeedsAccount(false); setError(undefined); setNotice(''); onPlace({ kind: 'cluster' }); }}>
        <option value="">Choose an installation</option>{targets.map(t => <option key={t.id} value={t.id}>{t.service_namespace} / {t.service_name} · {t.watched_namespace}</option>)}
      </select></>}
    {target && <>
      {connected ? <>
        <div className="cluster-result"><h2><code>{target.context_name}</code></h2><p><code>{target.watched_namespace}</code> · <code>{target.service_namespace} / {target.service_name}</code></p></div>
        <ul className="cluster-checks"><li><StatusIcon state="passed" /><b>Cluster found</b></li><li><StatusIcon state="passed" /><b role="status">Access confirmed</b></li></ul>
        <h3>AI connection</h3><label className="sr-only" htmlFor="send-provider">AI connection</label>
        <select className="field" ref={providerField} id="send-provider" value={providerId} onChange={event => { const next = event.target.value; setProviderId(next); providerName.current = access.providers.find(p => p.id === next)?.name ?? ''; onPlace({ kind: 'cluster', target, providers: access.providers, providerId: next }); }}>
          <option value="">Choose an AI connection</option>{access.providers.map(p => <option key={p.id} value={p.id}>{p.name} · {p.ready ? 'Ready' : 'Not ready'}</option>)}
        </select><p className="muted">Send checks whether this AI connection supports your agent's model.</p>
        <Button type="button" kind="primary" disabled={!providerId} title={!providerId ? 'Choose an AI connection first' : undefined} onClick={onOpenAgent}>Open your agent</Button>
        <Button type="button" kind="secondary" disabled={busy} onClick={() => { void confirm(); }}>Check access again</Button>
      </> : <>
        <div className="cluster-found"><b>Cluster found</b><span><code>{target.watched_namespace}</code> · <code>{target.service_namespace} / {target.service_name}</code></span></div>
        {!error && <p className="muted">Check that you can access this cluster.</p>}
        {needsAccount && <><p>This cluster needs an existing service account. Ask your cluster administrator for its name.</p>
          <label htmlFor="cluster-account">ServiceAccount name</label><input className="field" ref={accountField} id="cluster-account" autoComplete="off" spellCheck={false} disabled={busy} value={account} onChange={e => setAccount(e.target.value)} />
          <p className="muted">An existing account in {target.watched_namespace}. Bridge does not create accounts or permissions.</p></>}
        <Button type="button" kind="primary" disabled={busy || (needsAccount && !account.trim())} onClick={() => { void confirm(); }}>{error?.operation === 'Use cluster' ? 'Check again' : 'Use this cluster'}</Button>
      </>}
      <button type="button" className="workspace-disclosure" aria-expanded={details} aria-controls="cluster-details" onClick={() => setDetails(v => !v)}>{details ? 'Hide details' : 'Show details'}</button>
      {details && <dl id="cluster-details" className="cluster-details"><div><dt>Context</dt><dd>{target.context_name}</dd></div><div><dt>Service</dt><dd>{target.service_namespace} / {target.service_name}</dd></div><div><dt>Watched namespace</dt><dd>{target.watched_namespace}</dd></div><div><dt>Orka image</dt><dd>{target.image}</dd></div>
        {access?.providers.map(p => <div key={p.id}><dt>Provider</dt><dd>{p.name} <span className="muted">{p.ready ? 'Ready' : 'Not ready'}</span></dd></div>)}</dl>}
    </>}
    {error && recovery && <div className="workspace-failure" role="alert"><div className="workspace-failure-explanation">
      <p className="failure-summary">{recovery.cause}</p><p>{recovery.next}</p>
      <p>Your local cluster and agent files are kept. Connection checks do not create an Agent or run checks.</p>
      <div className="workspace-step-actions">
        {error.operation === 'Discover Orka' && <Button type="button" kind="primary" disabled={busy} onClick={() => { void check(contextId); }}>Check again</Button>}
        <Button type="button" kind="secondary" onClick={() => { void copy(); }}>Copy details for help</Button>
      </div>
    </div></div>}
    <Button type="button" kind="secondary" disabled={reloading} onClick={() => { void reload(); }}>{reloading ? 'Reloading kubeconfig…' : 'Reload kubeconfig'}</Button>
    <p className="muted">After reloading kubeconfig, choose a context and check access again.</p>
    {notice && <p role="status">{notice}</p>}
  </section>;
}
