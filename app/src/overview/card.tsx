import { useId, useState } from 'react';
import { Button } from '../components/Button';
import { GeneratedValue, StatusIcon, StatusPill } from '../components/DesignSystem';
import { Disclosure } from '../components/Disclosure';
import { RelativeTime } from '../components/RelativeTime';
import type { AgentRow, Card } from './model';

export type CardActions = { localAiReason?: string; onOpen: (id: string, clusterKey?: string) => void; onChat?: (id: string) => void; onRun?: (id: string, clusterKey?: string) => void };
function PlaceRow({ card, actions }: { card: Card; actions: CardActions }) {
  const [details, setDetails] = useState(false);
  const id = useId();
  const action = card.status === 'passed' ? card.actions.find(a => a.id === 'chat') ?? card.actions.find(a => a.id === 'run') : card.actions.find(a => a.id === 'run');
  const label = action?.id === 'chat' ? 'Chat' : 'Run tests';
  return <article className="overview-place" data-card-key={card.key} tabIndex={-1} aria-label={`${card.place}, ${card.pill ?? 'No recorded result'}`}>
    <div className="overview-place-head">
      <span className={card.remote ? 'overview-mono' : undefined}>{card.place}</span>
      {card.status ? <StatusPill state={card.status}>{card.pill}</StatusPill> : <span className="overview-note">No recorded result</span>}
      {card.meta.tests && <span className="overview-place-count">{card.meta.tests}</span>}
      {card.testedAt !== undefined && <span className="overview-time"><RelativeTime value={card.testedAt} /></span>}
      {action && <Button size="small" disabled={!card.remote && !!actions.localAiReason} aria-describedby={!card.remote ? actions.localAiReason : undefined} aria-label={`${label} ${card.subject}${card.remote ? ` on ${card.place}` : ''}`} onClick={() => {
        if (!card.documentId) return;
        if (action.id === 'chat') actions.onChat?.(card.documentId);
        else if (card.remote) actions.onRun?.(card.documentId, card.groupKey);
        else actions.onRun?.(card.documentId);
      }}>{label}</Button>}
      <Disclosure className="overview-details-toggle" label="Details" open={details} onOpenChange={setDetails} controls={id}>{null}</Disclosure>
    </div>
    {details && <div id={id} className="overview-details">{card.details.map(d => d.copy ? <GeneratedValue key={d.label} label={d.label} value={d.value} />
      : <div key={d.label} className="generated-value"><span>{d.label}</span><code>{d.value}</code></div>)}</div>}
  </article>;
}
export function AgentListRow({ row, actions, reveal, onToggle }: { row: AgentRow; actions: CardActions; reveal?: string; onToggle: () => void }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const revealed = !!reveal && row.cards.some(c => c.key === reveal);
  const expanded = open || revealed;
  const document = row.cards.find(c => !c.remote && c.documentId) ?? row.cards.find(c => c.documentId);
  // One mark per place, even when several distinct remote UIDs link to the same file.
  const marks = [...new Set(row.cards.map(c => c.groupKey))].map(key => {
    const cards = row.cards.filter(c => c.groupKey === key);
    return cards.find(c => c.status === 'failed') ?? cards.find(c => c.status === 'running') ?? cards.find(c => c.status === 'stale') ?? cards[0];
  });
  const toggle = () => { setOpen(!expanded); onToggle(); };
  return <article className="overview-agent" aria-label={row.title}>
    <header className="overview-agent-head">
      <h2 aria-label={row.title}><button type="button" className="overview-row-toggle" aria-label={`Show places for ${row.title}`} aria-describedby={`${id}-summary`} aria-expanded={expanded} aria-controls={id} onClick={toggle}>
        <span className="overview-agent-identity"><svg className={`icon${expanded ? ' overview-chevron-open' : ''}`} viewBox="0 0 24 24" aria-hidden="true"><path d="m9 5 7 7-7 7" /></svg>
          <span className="overview-agent-name"><span className="overview-agent-title">{row.title}</span><span className="overview-description">{row.description}</span></span>
        </span>
        <span className="overview-marks">{marks.map(card => <span key={card.groupKey} role="img" className="overview-mark" aria-label={`${card.place}: ${card.pill ?? 'No recorded result'}`} title={`${card.place}: ${card.pill ?? 'No recorded result'}`}>
          <StatusIcon state={card.status ?? 'not-started'} /><span className={card.remote ? 'overview-mono' : undefined}>{card.place}</span>
        </span>)}</span>
      </button></h2>
      <span id={`${id}-summary`} className="sr-only">{row.description} {marks.map(card => `${card.place}: ${card.pill ?? 'No recorded result'}`).join('; ')}</span>
      <span className="overview-time">{row.testedAt !== undefined && <RelativeTime value={row.testedAt} />}</span>
      {document ? <Button size="small" aria-label={`Open ${document.subject}`} onClick={() => {
        if (!document.documentId) return;
        if (document.remote) actions.onOpen(document.documentId, document.groupKey); else actions.onOpen(document.documentId);
      }}>Open</Button> : <Button size="small" aria-label={`Details ${row.title}`} aria-expanded={expanded} aria-controls={id} onClick={toggle}>Details</Button>}
    </header>
    {expanded && <div id={id} className="overview-places">{row.cards.map(card => <PlaceRow key={card.key} card={card} actions={actions} />)}</div>}
  </article>;
}
