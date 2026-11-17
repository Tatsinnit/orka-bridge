import { useEffect, useRef, useState, type RefObject } from 'react';
import { Button, CloseIcon } from '../components/Button';
import type { ContextView } from './api';

export default function ContextPicker({ choices, value, found, disabled, active, trigger, onChoose }: {
  choices: ContextView[]; value: string; found: Set<string>; disabled: boolean; active: boolean;
  trigger: RefObject<HTMLButtonElement | null>; onChoose: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const popover = useRef<HTMLDivElement>(null);
  function close(focus = true) { setOpen(false); setQuery(''); if (focus) trigger.current?.focus(); }
  useEffect(() => {
    if (!active || disabled) { setOpen(false); setQuery(''); return; }
    if (open) { popover.current?.showPopover?.(); popover.current?.querySelector('input')?.focus(); }
  }, [open, active, disabled]);
  const matches = choices.filter(choice => choice.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  return <>
    <label htmlFor="cluster-context">kubectl context</label>
    <Button ref={trigger} id="cluster-context" type="button" kind="secondary" className="context-trigger" aria-label="kubectl context" aria-describedby="cluster-context-value" aria-haspopup="dialog" aria-expanded={open && active && !disabled} aria-controls="cluster-context-picker" disabled={disabled}
      onClick={() => { if (open) close(); else setOpen(true); }}>
      <span id="cluster-context-value">{choices.find(choice => choice.id === value)?.name ?? 'Choose a context'}</span><span aria-hidden="true">⌄</span>
    </Button>
    {open && active && !disabled && <div ref={popover} popover="auto" role="dialog" aria-label="Choose a context" id="cluster-context-picker" className="context-picker"
      onToggle={event => { if (event.newState === 'closed') close(popover.current?.contains(document.activeElement) === true || document.activeElement === document.body); }}
      onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); close(); } }}>
      <Button type="button" kind="icon" className="picker-close" aria-label="Close picker" title="Close picker" onClick={() => close()}><CloseIcon /></Button>
      <label htmlFor="cluster-context-search">Find a context</label>
      <input id="cluster-context-search" className="field" type="search" value={query} onChange={event => setQuery(event.target.value)} />
      <ul>{matches.map(choice => <li key={choice.id}><button type="button" value={choice.id} aria-label={choice.name} aria-describedby={`context-${choice.id}`} aria-current={choice.id === value ? 'true' : undefined}
        onClick={() => { close(); onChoose(choice.id); }}>
        <span>{choice.name}</span><span id={`context-${choice.id}`} className="context-markers muted">{choice.current && <span>Current context</span>}{found.has(choice.id) && <span>Orka found</span>}</span>
      </button></li>)}</ul>
      {!matches.length && <p className="muted">{choices.length ? 'No matching contexts.' : 'No contexts available.'}</p>}
    </div>}
  </>;
}
