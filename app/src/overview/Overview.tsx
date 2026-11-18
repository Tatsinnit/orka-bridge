import { useEffect, useId, useRef, useState } from 'react';
import { Button } from '../components/Button';
import { type LiveAgents, type Observation, type Places, type RunAgent, readAgents, readFiles, readPlaces, type SavedFiles, unavailableAgents } from './api';
import { agentRows, buildCards, clusterGroups, type CompleteAgents, type Inputs, isEmpty, summaryLabel } from './model';
import { PlaceNotes } from './places';
import { AgentListRow } from './card';
import { type AgentFocus, sendAgentCheck } from '../clusters/send-api';
import './overview.css';

export type OverviewProps = {
  onNewAgent?: () => void; active: boolean; runBusy: boolean; localAiConnected: boolean; activeDocumentId?: string; workspaceKey: string;
  onOpenDocument: (id: string, clusterKey?: string) => void; onChatDocument?: (id: string) => void; onRunDocument?: (id: string, clusterKey?: string) => void;
  onOpenExample: () => void; onChooseWorkspace: () => void; onCounts?: (label: string) => void;
  /** Newest request wins. target is absent when native has no recorded agent for that Send. */
  focus?: { id: number; sendId: string; target?: AgentFocus };
  focusPending?: boolean;
  onRetryFocus?: (sendId: string) => void;
  workspaceNote?: string; filesRevision?: number; placesRevision?: number;
};
const focusNotes = { present: 'Not shown in Overview yet.', gone: 'The original agent is no longer available. Your saved result is unchanged.', cannotCheck: 'Cannot check this agent. Your saved result is unchanged.' };
const complete = (v?: LiveAgents): v is CompleteAgents => v?.state === 'complete' && v.installation !== null && v.observedAt !== null;
const hasRunningSend = (value?: Places) => value?.places.some(p => p.agents.some(a => a.record?.verdict === 'running')) || value?.unobserved.some(a => a.verdict === 'running');
export default function Overview({ onNewAgent, active, runBusy, localAiConnected, activeDocumentId, workspaceKey, onOpenDocument, onChatDocument, onRunDocument, onOpenExample, onChooseWorkspace, onCounts, workspaceNote, filesRevision = 0, placesRevision = 0, focus: navigationFocus, focusPending = false, onRetryFocus }: OverviewProps) {
  const aiReasonId = useId();
  const [dismissedFocus, setDismissedFocus] = useState<number>();
  const focus = navigationFocus?.id === dismissedFocus ? undefined : navigationFocus;
  const [focusRetry, setFocusRetry] = useState(0);
  // Last good reads are kept so a busy or failed refresh never blanks the page; each is flagged when it is not fresh.
  const [files, setFiles] = useState<SavedFiles>();
  const [filesStale, setFilesStale] = useState(false);
  const filesVisited = useRef(false);
  const finals = useRef(new Map<string, Observation>());
  const knownRunAgents = useRef(new Map<string, RunAgent>());
  const [live, setLive] = useState<LiveAgents>();
  const [last, setLast] = useState<CompleteAgents>();
  const [pending, setPending] = useState(false);
  const [localReady, setLocalReady] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [placeFilter, setPlaceFilter] = useState('all');
  const [nameFilter, setNameFilter] = useState('');
  const [reveal, setReveal] = useState<string>();
  const [places, setPlaces] = useState<Places>();
  const [placeError, setPlaceError] = useState(false);
  useEffect(() => {
    if (!active || focusPending || focus?.target) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function list() {
      const value = await readPlaces();
      if (cancelled) return;
      setPlaceError(!value);
      setPlaces(previous => value ?? (previous && { ...previous, places: previous.places.map(p => ({ ...p, stale: true })) }));
      // Poll known unfinished work; completion also invalidates a read that missed the initial checkpoint.
      if (hasRunningSend(value)) timer = setTimeout(() => { void list(); }, 500);
    }
    void list();
    return () => { cancelled = true; clearTimeout(timer); };
  }, [active, refresh, focusPending, focus?.target, placesRevision]);
  // See in Overview: refresh only the recorded destination, then focus the exact card (place key + UID, never a name).
  const focusEpoch = useRef(0);
  const toFocus = useRef<string | undefined>(undefined);
  const focusedCard = useRef<HTMLElement | undefined>(undefined);
  const autofocusRequest = useRef<{ identity: string; pending: boolean; completed: boolean } | undefined>(undefined);
  // The first read can fulfill navigation before its absence check has settled.
  const absencePending = useRef(false);
  const ownsFocus = useRef(false);
  useEffect(() => {
    const moved = (event: FocusEvent) => {
      if (ownsFocus.current && event.target !== focusedCard.current) { ownsFocus.current = false; toFocus.current = undefined; focusedCard.current = undefined; }
    };
    document.addEventListener('focusin', moved);
    return () => document.removeEventListener('focusin', moved);
  }, []);
  const [focusTick, setFocusTick] = useState(0);
  const [focusNote, setFocusNote] = useState<string>();
  useEffect(() => {
    if (!active || !focus) { setFocusNote(undefined); toFocus.current = undefined; focusedCard.current = undefined; autofocusRequest.current = undefined; absencePending.current = false; ownsFocus.current = false; return; }
    const request = ++focusEpoch.current;
    const identity = `${focus.id}/${focusRetry}`;
    const navigate = autofocusRequest.current?.identity !== identity || !!autofocusRequest.current?.pending && !focusPending;
    if (autofocusRequest.current?.identity !== identity) ownsFocus.current = true;
    const autofocus = { identity, pending: focusPending, completed: autofocusRequest.current?.identity === identity && autofocusRequest.current.completed };
    autofocusRequest.current = autofocus;
    // Completion invalidates the read, not the user's navigation, filters or disclosure choices.
    if (navigate) { setPlaceFilter('all'); setNameFilter(''); setReveal(undefined); setFocusNote(undefined); toFocus.current = undefined; focusedCard.current = undefined; absencePending.current = true; }
    if (focusPending) return;
    const { target, sendId } = focus;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let targetFound = false;
    async function list(navigate = false, resolveAbsence = false) {
      const value = target ? await readPlaces(target.key) : undefined;
      if (request !== focusEpoch.current) return;
      if (navigate) autofocus.completed = true;
      if (target) setPlaceError(!value);
      const seen = value?.places.find(p => p.key === target?.key);
      // Other places were not queried by this refresh, so keep their cached freshness.
      setPlaces(previous => value ?? (target && previous && { ...previous, places: previous.places.map(p => p.key === target.key ? { ...p, stale: true } : p) }) ?? previous);
      // Scoped reads include other places' cached records; only this destination or Send can keep them polling.
      if (target && (seen?.agents.some(a => a.record?.verdict === 'running') || value?.unobserved.some(a => a.id === sendId && a.verdict === 'running'))) timer = setTimeout(() => { void list(); }, 500);
      // Presence supersedes the initial check; polling still preserves disclosure and focus choices.
      if (target && seen?.agents.some(a => a.uid === target.uid && a.namespace === target.namespace)) {
        targetFound = true; absencePending.current = false; setFocusNote(undefined);
        if (navigate) { toFocus.current = `${target.key}/${target.uid}`; setReveal(toFocus.current); setFocusTick(v => v + 1); }
        return;
      }
      if (!resolveAbsence) return;
      const check = target ? await sendAgentCheck(sendId) : 'cannotCheck';
      if (request === focusEpoch.current && !targetFound) { absencePending.current = false; setFocusNote(focusNotes[check]); }
    }
    // Resume either unfinished step without repeating navigation's initialization resets.
    void list(!autofocus.completed, absencePending.current);
    return () => { focusEpoch.current++; clearTimeout(timer); };
  }, [active, focus, focusRetry, focusPending, placesRevision]);
  useEffect(() => {
    // A navigation request reveals its exact place/UID before focus moves.
    const key = toFocus.current; if (!active || !localReady || !ownsFocus.current || !key) return;
    if (focusedCard.current?.isConnected) return;
    if (focusedCard.current && document.activeElement !== document.body) { toFocus.current = undefined; focusedCard.current = undefined; return; }
    const card = [...document.querySelectorAll<HTMLElement>('[data-card-key]')].find(node => node.dataset.cardKey === key);
    // Retain ownership across post-run rebalancing, but never override a later user focus.
    if (card) { focusedCard.current = card; card.focus(); card.scrollIntoView?.({ block: 'center' }); }
  }, [active, localReady, focusTick, places, files, live, reveal]);
  const epoch = useRef(0);
  const knownWorkspace = useRef(workspaceKey);
  useEffect(() => {
    const request = ++epoch.current;
    if (knownWorkspace.current !== workspaceKey) {
      knownWorkspace.current = workspaceKey; setLive(undefined); setLast(undefined);
    }
    if (!active) return;
    filesVisited.current = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    setPending(runBusy); setLocalReady(false);
    async function list(retried = false) {
      const value = await readAgents();
      if (request !== epoch.current) return;
      setLive(value);
      if (complete(value)) { setLast(value); setPending(false); setLocalReady(true); }
      else if (value.state === 'busy' && value.runActive) { setPending(true); setLocalReady(true); }
      else if (value.state === 'busy' && !retried) timer = setTimeout(() => { void list(true); }, 250);
      else setLocalReady(true);
    }
    void readFiles().then(value => {
      if (request !== epoch.current) return;
      if (value.state === 'complete') {
        value.files.forEach(f => { if (f.localResult && (f.localResult.state === 'complete' || f.localResult.state === 'unknown' && (!(runBusy || value.runActive) || f.documentId !== activeDocumentId))) finals.current.set(f.documentId, f.localResult); });
        // Cleanup may forget a removed Agent while the cached inventory still contains it.
        value.runAgents.forEach(a => knownRunAgents.current.set(JSON.stringify([a.installation, a.namespace, a.agentUid]), a));
        setFiles({ ...value, runAgents: [...knownRunAgents.current.values()] }); setFilesStale(false);
      } else {
        setFiles(previous => previous?.state === 'complete' ? previous : value); setFilesStale(true);
        // Keep the last coherent inventory; fresh Agents cannot be filtered against stale run ownership.
        setLive(unavailableAgents); setLocalReady(true); return;
      }
      if (runBusy || value.runActive) { setPending(true); setLocalReady(true); return; }
      void list();
    });
    return () => { epoch.current++; clearTimeout(timer); };
  }, [active, runBusy, activeDocumentId, workspaceKey, refresh, filesRevision]);
  // The hidden tab reads saved files only, initially and after a registration/workspace change.
  // Live Agents and remembered clusters are still observed only when Overview is visited.
  useEffect(() => {
    if (active) return;
    const request = epoch.current;
    let cancelled = false;
    void readFiles().then(value => {
      if (cancelled || request !== epoch.current) return;
      if (value.state !== 'complete') { setFiles(previous => previous?.state === 'complete' ? previous : value); setFilesStale(true); return; }
      value.files.forEach(f => { if (f.localResult && (f.localResult.state === 'complete' || f.localResult.state === 'unknown' && (!(runBusy || value.runActive) || f.documentId !== activeDocumentId))) finals.current.set(f.documentId, f.localResult); });
      setPending(runBusy || value.runActive);
      value.runAgents.forEach(a => { knownRunAgents.current.set(JSON.stringify([a.installation, a.namespace, a.agentUid]), a); });
      setFiles({ ...value, runAgents: [...knownRunAgents.current.values()] }); setFilesStale(false);
    });
    return () => { cancelled = true; };
  }, [active, filesRevision, workspaceKey, runBusy, activeDocumentId]);
  const observed = complete(live) ? live : last;
  const inputs: Inputs = { files, filesStale, finals: finals.current, live, observed, pending, activeDocumentId, places, placeError, chat: !!onChatDocument, run: !!onRunDocument, workspaceNote };
  const built = buildCards(inputs);
  const empty = isEmpty(inputs, built);
  const partial = !!workspaceNote || filesStale || files?.historyState !== 'complete' || !complete(live) || pending || !places || placeError || places.places.some(p => p.stale || p.observedAt === null);
  const counts = !files ? 'Reading saved files…' : files.state === 'unavailable' ? 'Saved files unavailable'
    : !filesVisited.current ? filesStale ? 'Saved files unavailable' : files.files.length ? `${files.files.length} saved agent${files.files.length === 1 ? '' : 's'}` : 'No saved agents yet'
    : summaryLabel(agentRows(built.cards), partial);
  useEffect(() => { onCounts?.(counts); }, [counts, onCounts]);
  const clusters = clusterGroups(inputs, built);
  const knownPlace = placeFilter === 'all' || placeFilter === 'local' || !!places?.places.some(place => place.key === placeFilter);
  const selectedPlace = knownPlace ? placeFilter : 'all';
  useEffect(() => { if (places && !knownPlace) setPlaceFilter('all'); }, [places, knownPlace]);
  const rows = agentRows(built.cards, selectedPlace, nameFilter);
  const options = [{ key: 'local', label: 'This computer' }, ...(places?.places ?? []).map(p => ({ key: p.key, label: built.labels.get(p.key)! }))];
  const needsAi = !localAiConnected && built.local.some(c => c.actions.some(a => a.id !== 'open'));
  const actions = { localAiReason: needsAi ? aiReasonId : undefined, onOpen: onOpenDocument, onChat: onChatDocument, onRun: onRunDocument, onCheck: () => { setDismissedFocus(navigationFocus?.id); setRefresh(v => v + 1); }, onClusters: onChooseWorkspace };
  return <section className="overview" aria-label="Your agents" hidden={!active}>
    <div className="page-heading"><h1 id={active ? 'page-title' : undefined}>Your agents</h1><div className="overview-actions">
      {onNewAgent && !empty && <Button kind="primary" onClick={onNewAgent}>New agent</Button>}
      </div></div>
    {!empty && <div className="overview-filters">
      {options.length > 4 ? <select className="field" aria-label="Place" value={selectedPlace} onChange={event => { setReveal(undefined); setPlaceFilter(event.target.value); }}><option value="all">All places</option>{options.map(p => <option key={p.key} value={p.key}>{p.label}</option>)}</select>
        : <div className="view-switch" role="group" aria-label="Place">{[{ key: 'all', label: 'All' }, ...options].map(p => <button key={p.key} type="button" aria-pressed={selectedPlace === p.key} onClick={() => { setReveal(undefined); setPlaceFilter(p.key); }}>{p.label}</button>)}</div>}
      <input className="field" aria-label="Filter by name" placeholder="Filter by name" value={nameFilter} onChange={event => { setReveal(undefined); setNameFilter(event.target.value); }} />
      <span className="overview-filter-count" role="status">{summaryLabel(rows, partial)}</span>
    </div>}
    {needsAi && <p id={aiReasonId} className="overview-note" tabIndex={0}>Connect the AI in Clusters to chat or run tests on this computer.</p>}
    {focusNote && <div className="overview-note" role="status"><p>{focusNote}</p><Button kind="quiet" onClick={() => { if (focus && !focus.target && onRetryFocus) onRetryFocus(focus.sendId); else setFocusRetry(v => v + 1); }}>Refresh</Button></div>}
    {placeError && <div className="overview-note" role="status"><p>Couldn't read clusters just now.{places && ' Showing what Bridge last saw.'}</p><Button kind="quiet" onClick={actions.onCheck}>Check again</Button></div>}
    {places?.historyState === 'unavailable' && <div role="alert" className="overview-note"><p>Saved Send results are unavailable, so cluster agents show no test result.</p><Button kind="quiet" onClick={actions.onCheck}>Check again</Button></div>}
    <PlaceNotes groups={clusters} actions={actions} />
    {empty ? <>
      <div className="card overview-empty"><h2>No agents yet</h2><p>Make an agent, or start from web-reader, the example that answers from a web page. Agents you test or send show up here, with their results in each place.</p>
        <div className="overview-card-actions">{onNewAgent && <Button kind="primary" onClick={onNewAgent}>New agent</Button>}<Button onClick={onOpenExample}>Open web-reader</Button></div></div>
    </> : <div className="overview-list">{rows.map(row => <AgentListRow key={row.key} row={row} actions={actions} reveal={reveal} onToggle={() => setReveal(undefined)} />)}{!rows.length && built.cards.length > 0 && <p className="overview-note">No agents match these filters.</p>}</div>}
  </section>;
}
