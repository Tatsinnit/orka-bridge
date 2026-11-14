import { Button } from '../components/Button';
import { useEffect, useRef, useState } from 'react';
import { createDocument, creationRoot } from './documents-api';

export default function NewAgent({ active, model, onRegistered, onCreated, onCancel }: { active: boolean; model?: string; onRegistered: () => void; onCreated: (id: string) => Promise<void>; onCancel: () => void }) {
  const [name, setName] = useState('');
  const [root, setRoot] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const admitted = useRef(false);
  const input = useRef<HTMLInputElement>(null);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; input.current?.focus(); void creationRoot().then(value => { if (mounted.current) setRoot(value); }); return () => { mounted.current = false; }; }, []);
  const stem = name || '<name>';
  const separator = root?.includes('\\') ? '\\' : '/';
  return <section className="new-agent" aria-labelledby={active ? 'page-title' : undefined}>
    <div className="page-heading"><h1 id={active ? 'page-title' : undefined}>New agent</h1></div>
    <form className="new-agent-form card" onSubmit={event => {
      event.preventDefault(); if (admitted.current) return;
      admitted.current = true; setBusy(true); setError(undefined);
      // An explicit Open can leave this form while native creation finishes.
      // App still refreshes registration, but a late reply cannot steal selection.
      void createDocument(name, model).then(id => { onRegistered(); if (mounted.current) return onCreated(id); }).catch(() => {
        if (mounted.current) setError('Your agent could not be created. Check the name and folder, or choose another name if it already exists, then try again.');
      }).finally(() => { admitted.current = false; if (mounted.current) setBusy(false); });
    }}>
      <label htmlFor="new-agent-name">Name</label>
      <input id="new-agent-name" className="field" ref={input} autoCapitalize="none" autoCorrect="off" spellCheck={false} required minLength={1} maxLength={63} pattern="[a-z0-9]([a-z0-9\-]{0,61}[a-z0-9])?" aria-describedby="new-agent-name-hint" value={name} disabled={busy} onChange={event => { setName(event.target.value); setError(undefined); }} />
      <p className="muted" id="new-agent-name-hint">Use 1–63 lowercase letters, digits or hyphens. Begin and end with a letter or digit. Windows device names and web-reader are reserved. The name is used in the files, not as the Kubernetes resource name.</p>
      <fieldset><legend>Keep it in</legend><label><input type="radio" checked readOnly /> Bridge's folder</label></fieldset>
      <div className="new-agent-path">
        <p>{root ? `${root}${separator}${stem}${separator}` : `~/Orka/agents/${stem}/`}</p>
        <ul><li>{stem}.agent.md</li><li>{stem}.agent.test.md</li></ul>
      </div>
      <p className="muted">Saved on this computer with local Git history. Nothing is uploaded.</p>
      <p>Starts with instructions and a check that fetches a web page. Change instructions in Designer; edit the check file on disk.</p>
      {error && <p role="alert">{error}</p>}
      <div className="heading-actions"><Button kind="primary" type="submit" disabled={busy}>{busy ? 'Creating…' : 'Create'}</Button><Button kind="secondary" type="button" disabled={busy} onClick={onCancel}>Cancel</Button></div>
    </form>
  </section>;
}
