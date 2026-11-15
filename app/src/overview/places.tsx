import { useEffect, useState } from 'react';
import { Button } from '../components/Button';
import { RelativeTime } from '../components/RelativeTime';
import { AgentCard, type CardActions } from './card';
import type { Group } from './model';

// Compact (under 820 px) is one column in logical order. The wide layout is two independent stacks, so the switch is a
// render choice rather than CSS reordering: keyboard order always matches what is on screen.
const query = '(max-width: 820px)';
export function useCompact() {
  const [compact, setCompact] = useState(() => typeof matchMedia === 'function' && matchMedia(query).matches);
  useEffect(() => {
    if (typeof matchMedia !== 'function') return;
    const media = matchMedia(query), update = () => setCompact(media.matches);
    update(); media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  return compact;
}
export type GroupActions = CardActions & { onCheck: () => void; onClusters: () => void };
export function GroupSection({ group, byAgent, actions }: { group: Group; byAgent: boolean; actions: GroupActions }) {
  return <section className="overview-group" aria-label={group.title}>
    <header className="overview-group-head">
      <h2 className={group.key === 'local' ? undefined : 'overview-mono'}>{group.title}</h2>
      {group.sub && <span className="overview-group-sub">{group.sub}</span>}
      {group.count !== undefined && <span className="overview-group-count">{group.count} {group.count === 1 ? 'agent' : 'agents'}</span>}
    </header>
    {group.description && <p className="overview-group-description">{group.description}</p>}
    {group.notes.map(note => <div key={note.text} className="overview-note" role="status">
      <p>{note.text}{note.seen !== undefined && <> Showing what Bridge saw <RelativeTime value={note.seen} />.</>}</p>
      {note.check && <Button kind="quiet" onClick={actions.onCheck}>Check again</Button>}
      {note.clusters && <Button kind="quiet" onClick={actions.onClusters}>Open Clusters</Button>}
    </div>)}
    {group.cards.map(card => <AgentCard key={card.key} card={card} byAgent={byAgent && group.key !== 'unlinked'} actions={actions} />)}
  </section>;
}
export function Board({ stacks, byAgent, actions }: { stacks: Group[][]; byAgent: boolean; actions: GroupActions }) {
  return <div className="overview-board">{stacks.map((stack, i) => <div key={i} className="overview-stack">{stack.map(g => <GroupSection key={g.key} group={g} byAgent={byAgent} actions={actions} />)}</div>)}</div>;
}
