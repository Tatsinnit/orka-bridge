import { useEffect, useRef, useState } from 'react';
import { Button } from '../components/Button';
import { storageCopy } from './agent-api';
import { createDocument, creationRoot, pickRepo, type RepoSelection } from './documents-api';

export default function NewAgent({ active, model, onRegistered, onCreated, onCancel }: { active: boolean; model?: string; onRegistered: () => void; onCreated: (id: string) => Promise<void>; onCancel: () => void }) {
  const [name, setName] = useState('');
  const [root, setRoot] = useState<string>();
  const [storage, setStorage] = useState<'bridge' | 'repo'>('bridge');
  const [repo, setRepo] = useState<RepoSelection>();
  const [picking, setPicking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const admitted = useRef(false);
  const input = useRef<HTMLInputElement>(null);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; input.current?.focus(); void creationRoot().then(value => { if (mounted.current) setRoot(value); }); return () => { mounted.current = false; }; }, []);
  const stem = name || '<name>';
  const destination = storage === 'bridge' ? root : repo?.path;
  const separator = destination?.includes('\\') ? '\\' : '/';
  const disabled = busy || picking;
  async function chooseRepo() {
    if (admitted.current) return;
    admitted.current = true; setPicking(true); setError(undefined);
    try { const selected = await pickRepo(); if (selected && mounted.current) setRepo(selected); }
    catch (failure) { if (mounted.current) setError((failure as Error).message); }
    finally { admitted.current = false; if (mounted.current) setPicking(false); }
  }
  return <section className="new-agent" aria-labelledby={active ? 'page-title' : undefined}>
    <div className="page-heading"><h1 id={active ? 'page-title' : undefined}>New agent</h1></div>
    <form className="new-agent-form card" onSubmit={event => {
      event.preventDefault(); if (admitted.current || storage === 'repo' && !repo) return;
      admitted.current = true; setBusy(true); setError(undefined);
      // An explicit Open can leave this form while native creation finishes.
      // App still refreshes registration, but a late reply cannot steal selection.
      void createDocument(name, model, storage === 'repo' ? repo?.id : undefined).then(id => { onRegistered(); if (mounted.current) return onCreated(id); }).catch(() => {
        if (mounted.current) setError('Your agent could not be created. Check the name and folder, or choose another name if it already exists, then try again.');
      }).finally(() => { admitted.current = false; if (mounted.current) setBusy(false); });
    }}>
      <label htmlFor="new-agent-name">Name</label>
      <input id="new-agent-name" className="field" ref={input} autoCapitalize="none" autoCorrect="off" spellCheck={false} required minLength={1} maxLength={63} pattern="[a-z0-9]([a-z0-9\-]{0,61}[a-z0-9])?" aria-describedby="new-agent-name-hint" value={name} disabled={disabled} onChange={event => { setName(event.target.value); setError(undefined); }} />
      <p className="muted" id="new-agent-name-hint">Use 1–63 lowercase letters, digits or hyphens. Begin and end with a letter or digit. Windows device names{storage === 'bridge' ? ' and web-reader are' : ' are'} reserved. The name is used in the files, not as the Kubernetes resource name.</p>
      <fieldset disabled={disabled}><legend>Keep it in</legend>
        <label><input type="radio" name="agent-storage" checked={storage === 'bridge'} onChange={() => { setStorage('bridge'); setError(undefined); }} /> Bridge's folder</label>
        <label><input type="radio" name="agent-storage" checked={storage === 'repo'} onChange={() => { setStorage('repo'); setError(undefined); }} /> Your own repo</label>
      </fieldset>
      {storage === 'repo' && <><div className="heading-actions"><Button kind="secondary" type="button" disabled={disabled} onClick={() => { void chooseRepo(); }}>{picking ? 'Choosing…' : repo ? 'Change repo' : 'Choose repo'}</Button></div>
        {!repo && <p className="muted">Choose an existing local Git working checkout.</p>}</>}
      <div className="new-agent-path">
        {(destination || storage === 'bridge') && <p>{destination ? `${destination.replace(/[\\/]+$/, '')}${separator}${stem}${separator}` : `~/Orka/agents/${stem}/`}</p>}
        <ul><li>{stem}.agent.md</li><li>{stem}.agent.test.md</li></ul>
      </div>
      <p className="muted">{storageCopy[storage]}</p>
      <p>Starts with instructions and a check that fetches a web page. Change instructions in Designer; edit the check file on disk.</p>
      {error && <p role="alert">{error}</p>}
      <div className="heading-actions"><Button kind="primary" type="submit" disabled={disabled || storage === 'repo' && !repo}>{busy ? 'Creating…' : 'Create'}</Button><Button kind="secondary" type="button" disabled={disabled} onClick={onCancel}>Cancel</Button></div>
    </form>
  </section>;
}
