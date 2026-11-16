import { type ReactNode, useId, useState } from 'react';
import { Button } from '../components/Button';
import { GeneratedValue, StatusPill } from '../components/DesignSystem';
import { Disclosure } from '../components/Disclosure';
import { RelativeTime } from '../components/RelativeTime';
import type { Card } from './model';

export type CardActions = { onOpen: (id: string) => void; onChat?: (id: string) => void; onRun?: (id: string) => void };
const mono = (text: string) => <code>{text}</code>;
const names = (list: string[]) => list.map((n, i) => <span key={n}>{i ? i === list.length - 1 ? ' and ' : ', ' : ''}{mono(n)}</span>);
function Meta({ card }: { card: Card }) {
  const { tests, model, tools, also, on } = card.meta;
  const parts: ReactNode[] = [tests, model && mono(model), tools && mono(tools)].filter(Boolean);
  // Two or fewer names fit on the line; more become a count.
  if (also.length) parts.push(<>also on {also.length > 2 ? `${also.length} clusters` : names(also)}</>);
  if (on) parts.push(<>on {mono(on)}</>);
  return parts.length ? <p className="overview-card-meta">{parts.map((p, i) => <span key={i}>{i ? ' · ' : ''}{p}</span>)}</p> : null;
}
export function AgentCard({ card, byAgent, actions }: { card: Card; byAgent: boolean; actions: CardActions }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const label = (verb: string) => `${verb} ${card.subject}${card.remote ? ` on ${card.place}` : ''}`;
  const run = (kind: 'open' | 'chat' | 'run') => { if (card.documentId) ({ open: actions.onOpen, chat: actions.onChat, run: actions.onRun })[kind]?.(card.documentId); };
  const title = byAgent ? card.place : card.title;
  return <article className="card overview-card" aria-label={`${title}${card.pill ? `, ${card.pill}` : ''}`}>
    <header className="overview-card-head">
      <h3>{title}</h3>
      {card.status && <StatusPill state={card.status}>{card.pill}</StatusPill>}
      {card.testedAt !== undefined && <span className="overview-card-time">tested <RelativeTime value={card.testedAt} /></span>}
    </header>
    {/* In By agent the group head already shows a linked file's description. */}
    {!(byAgent && !card.agentKey.startsWith('agent/')) && <p className="overview-card-description">{card.description}</p>}
    <Meta card={card} />
    <div className="overview-card-actions">
      {card.actions.map(a => <Button key={a.id} kind={a.primary ? 'primary' : 'secondary'} aria-label={label({ open: 'Open', chat: 'Chat', run: 'Run tests' }[a.id])} onClick={() => run(a.id)}>{{ open: 'Open', chat: 'Chat', run: 'Run tests' }[a.id]}</Button>)}
      <Disclosure className="overview-details-toggle" label="Details" open={open} onOpenChange={setOpen} controls={id}>{null}</Disclosure>
    </div>
    {open && <div id={id} className="overview-details">{card.details.map(d => d.copy ? <GeneratedValue key={d.label} label={d.label} value={d.value} />
      : <div key={d.label} className="generated-value"><span>{d.label}</span><code>{d.value}</code></div>)}</div>}
  </article>;
}
