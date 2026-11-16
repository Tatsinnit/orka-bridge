import type { Status } from '../components/DesignSystem';
import { sameSource } from '../designer/agent-api';
import type { LiveAgents, Observation, Place, Places, SavedFile, SavedFiles } from './api';

export type CardAction = { id: 'open' | 'chat' | 'run'; primary?: boolean };
export type Detail = { label: string; value: string; copy?: boolean };
export type Card = {
  key: string; groupKey: string; agentKey: string; title: string; place: string; subject: string;
  status?: Status; pill?: string; testedAt?: number; description: string;
  meta: { tests?: string; model?: string; tools?: string; also: string[]; on?: string };
  documentId?: string; remote: boolean; actions: CardAction[]; details: Detail[];
};
export type Note = { text: string; seen?: number; check?: boolean; clusters?: boolean };
export type Group = { key: string; title: string; sub?: string; description?: string; count?: number; notes: Note[]; cards: Card[] };
export type CompleteAgents = LiveAgents & { state: 'complete'; installation: string; observedAt: number };
export type Inputs = {
  files?: SavedFiles; filesStale: boolean; finals: Map<string, Observation>;
  live?: LiveAgents; observed?: CompleteAgents; pending: boolean; activeDocumentId?: string;
  places?: Places; placeError: boolean; chat: boolean; run: boolean; workspaceNote?: string;
};
export type Built = ReturnType<typeof buildCards>;

const exact = (...parts: string[]) => JSON.stringify(parts);
const absolute = (ms: number) => new Date(ms).toLocaleString();
const sourceKey = (s: { agent: string; tests: string }) => `${s.agent}/${s.tests}`;
const fileTitle = (f: SavedFile) => f.summary?.name || f.name;
// Context name alone when unique, then with namespace, then with the service, so two installations never look alike.
function placeLabels(places: Place[]) {
  const labels = new Map<string, string>();
  const clash = (key: (p: Place) => string, p: Place) => places.filter(q => key(q) === key(p)).length > 1;
  places.forEach(p => labels.set(p.key, !clash(q => q.context, p) ? p.context : !clash(q => `${q.context}/${q.namespace}`, p) ? `${p.context} · ${p.namespace}` : `${p.context} · ${p.namespace} · ${p.service}`));
  return labels;
}
function ownedFile(source: { agent: string; tests: string }, files: SavedFile[]) {
  const matches = files.filter(f => f.state === 'readable' && f.source && sourceKey(f.source) === sourceKey(source));
  return matches.length === 1 ? matches[0] : undefined;
}
function localState(live: LiveAgents | undefined, pending: boolean) {
  if (pending) return 'updating';
  if (!live) return 'reading';
  if (live.state === 'busy') return 'busy';
  if (live.state === 'unavailable') return live.reason ?? 'read';
  return 'ok';
}
// Last result for a file: the native record, or the exact live observation when it is newer. A run in progress keeps
// the previous final result for the session; with none the result is unknown (Running is never shown for a file).
function resultFor(file: SavedFile, observed: CompleteAgents | undefined, finals: Map<string, Observation>, active: boolean) {
  let result = file.localResult ?? undefined;
  const live = observed?.observation;
  if (live?.documentId === file.documentId && (!result || live.observedAt >= result.observedAt)) result = live;
  // Native history reads normalize an in-progress persisted row to Unknown, even while the test is active.
  if (result?.state === 'running' || active && result?.state === 'unknown') return { result: finals.get(file.documentId), running: true };
  return { result, running: false };
}
type Verdict = { status: Status; pill: string; earlier?: boolean };
function localStatus(file: SavedFile, result: Observation | undefined, running: boolean, historyOk: boolean): Verdict {
  if (!result) return running || !historyOk ? { status: 'stale', pill: 'Result unknown' } : { status: 'not-started', pill: 'Not tested' };
  const current = !!file.source && sameSource(file.source, result.source) === true;
  const verdict = result.state === 'complete' ? result.report?.verdict : undefined;
  // A failure on earlier files is still a failure; a pass on earlier files says nothing about the saved one.
  if (verdict === 'failed') return { status: 'failed', pill: 'Failed', earlier: !current };
  if (!file.source) return { status: 'stale', pill: 'Result unknown' };
  if (!current) return { status: 'stale', pill: 'Not run since your edit' };
  if (verdict === 'passed') return { status: 'passed', pill: 'Passed' };
  if (verdict === 'cancelled') return { status: 'not-started', pill: 'Not tested' };
  return { status: 'stale', pill: 'Result unknown' };
}
// Keep the same action order as the frame, even when the next step changes.
function localActions(file: SavedFile, status: Status, chat: boolean, run: boolean): CardAction[] {
  if (file.state !== 'readable') return [];
  return [
    { id: 'open', primary: status === 'failed' },
    ...(chat && status !== 'failed' ? [{ id: 'chat' as const }] : []),
    ...(run ? [{ id: 'run' as const, primary: status !== 'passed' && status !== 'failed' }] : []),
  ];
}
const toolText = (tools?: string[] | null) => tools ? tools.join(', ') || 'no tools' : undefined;

export function buildCards(i: Inputs) {
  const files = i.files?.state === 'complete' ? i.files : undefined;
  const saved = files?.files ?? [];
  const ownershipKnown = !!files && files.historyState === 'complete';
  const history = ownershipKnown && !i.filesStale;
  const state = i.workspaceNote ? 'workspace' : localState(i.live, i.pending);
  const acts = state === 'ok' || state === 'image';
  const labels = placeLabels(i.places?.places ?? []);
  // A remote card links to a file only on exactly one saved hash match (document association is PR 3). The link is
  // then a document, so "also on" never compares hashes directly.
  const linked = new Map<string, string[]>();
  const remote = (i.places?.places ?? []).flatMap(place => place.agents.map(agent => {
    const record = i.places?.historyState === 'complete' ? agent.record : null;
    const file = record ? ownedFile(record.source, saved) : undefined;
    if (file) linked.set(file.documentId, [...(linked.get(file.documentId) ?? []), place.key]);
    return { place, agent, record, file };
  }));
  const where = (documentId: string, except?: string) => [...new Set(linked.get(documentId) ?? [])].filter(k => k !== except).map(k => labels.get(k)!);

  const local: Card[] = saved.map(file => {
    const { result, running } = resultFor(file, i.observed, i.finals, i.pending && i.activeDocumentId === file.documentId);
    const s = localStatus(file, result, running, history);
    const counts = result?.report && (s.status === 'passed' || s.status === 'failed') ? `${result.report.passed} of ${result.report.total} tests${s.earlier ? ' on earlier files' : ''}` : undefined;
    const details: Detail[] = [];
    if (file.source) details.push({ label: 'Agent file hash', value: file.source.agent, copy: true }, { label: 'Test file hash', value: file.source.tests, copy: true });
    if (result) details.push({ label: 'Tested', value: absolute(result.observedAt) }, { label: 'Test run Agent', value: result.agentName }, { label: 'Test run Agent ID', value: result.agentUid, copy: true });
    return {
      key: `local/${file.documentId}`, groupKey: 'local', agentKey: file.documentId, title: fileTitle(file), place: 'This computer', subject: file.name,
      status: s.status, pill: s.pill, testedAt: result?.observedAt, description: file.summary?.description || file.reason || 'No description in the agent file.',
      meta: { tests: counts, model: file.summary?.model, tools: toolText(file.summary?.tools), also: where(file.documentId) },
      documentId: file.state === 'readable' ? file.documentId : undefined, remote: false, actions: localActions(file, s.status, i.chat && acts, i.run && acts), details,
    };
  });
  // A stale files read keeps its matching cached inventory; Overview never pairs it with newly observed Agents.
  // Names never decide: a Bridge test run is an exact
  // installation + namespace + UID in the recorded list (or the live observation).
  const owned = new Set([...(files?.runAgents ?? []).map(a => exact(a.installation, a.namespace, a.agentUid)),
    ...(i.observed?.observation ? [exact(i.observed.installation, i.observed.observation.namespace, i.observed.observation.agentUid)] : [])]);
  // Chat's generated Agents are implementation details, not the creator's files. This is display-only;
  // it must never be used to claim ownership or authorize resource cleanup.
  const strays: Card[] = !ownershipKnown || !i.observed ? [] : i.observed.agents.filter(a => !/^bridge-chat-agent-[0-9a-f]{32}$/.test(a.name) && !owned.has(exact(i.observed!.installation, a.namespace, a.uid))).map(a => ({
    key: `local/${exact(a.namespace, a.uid)}`, groupKey: 'local', agentKey: `agent/local/${a.namespace}/${a.uid}`, title: a.name, place: 'This computer', subject: a.name,
    description: 'No agent file on this computer.', meta: { also: [], on: 'This computer' }, remote: false, actions: [],
    details: [{ label: 'Name on the cluster', value: a.name }, { label: 'Namespace', value: a.namespace }, { label: 'Agent ID', value: a.uid, copy: true }, { label: 'Last seen', value: absolute(i.observed!.observedAt) }],
  }));
  const unavailable = i.places?.historyState === 'unavailable';
  const remoteCards: Card[] = remote.map(({ place, agent, record, file }) => {
    const s: { status?: Status; pill?: string } = unavailable ? { status: 'needs-you', pill: "Couldn't check" } : !record ? {}
      : place.stale || record.verdict === 'unknown' ? { status: 'stale', pill: 'Result unknown' }
        : record.verdict === 'running' ? { status: 'running', pill: 'Running' } : record.verdict === 'cancelled' ? { status: 'not-started', pill: 'Not tested' } : { status: record.verdict, pill: record.verdict === 'passed' ? 'Passed' : 'Failed' };
    const name = labels.get(place.key)!;
    const details: Detail[] = [{ label: 'Name on the cluster', value: agent.name }, { label: 'Namespace', value: place.namespace }, { label: 'Agent ID', value: agent.uid, copy: true }, { label: 'Cluster', value: `${place.context} · ${place.service}` }];
    if (record) details.push({ label: 'Agent file hash', value: record.source.agent, copy: true }, { label: 'Test file hash', value: record.source.tests, copy: true });
    if (place.observedAt !== null) details.push({ label: 'Last seen', value: absolute(place.observedAt) });
    return {
      key: `${place.key}/${agent.uid}`, groupKey: place.key, agentKey: file?.documentId ?? `agent/${place.key}/${agent.uid}`, title: file ? fileTitle(file) : agent.name, place: name, subject: file?.name ?? agent.name,
      ...s, testedAt: record ? record.time * 1000 : undefined, description: file ? file.summary?.description || 'No description in the agent file.' : 'No agent file on this computer.',
      meta: { tests: record && place.stale ? `Last Send ${record.verdict}` : undefined, model: agent.model ?? file?.summary?.model, tools: toolText(agent.tools ?? file?.summary?.tools),
        also: file ? ['this computer', ...where(file.documentId, place.key)] : [], on: file ? undefined : name },
      documentId: file?.state === 'readable' ? file.documentId : undefined, remote: true, actions: file ? [{ id: 'open', primary: s.status === 'failed' }] : [], details,
    };
  });
  return { cards: [...local, ...strays, ...remoteCards], local, strays, remoteCards, saved, history, state, labels };
}

export function clusterGroups(i: Inputs, built: Built): Group[] {
  const { state, history, strays, local, labels } = built;
  const notes: Note[] = [];
  const seen = i.observed?.observedAt;
  if (i.files?.state === 'unavailable') notes.push({ text: "Couldn't read your saved files.", check: true });
  else if (i.filesStale) notes.push({ text: "Couldn't read your saved files just now. Showing what Bridge last read.", check: true });
  if (!i.files) notes.push({ text: 'Reading saved files…' });
  if (i.workspaceNote) notes.push({ text: i.workspaceNote, clusters: true });
  if (state === 'reading') notes.push({ text: 'Reading agents…' });
  if (state === 'updating') notes.push({ text: 'Updating when the test finishes.', seen });
  if (state === 'busy') notes.push({ text: 'The local cluster is busy right now.', seen, check: true });
  if (state === 'stopped') notes.push({ text: 'The local cluster is stopped. Start it to test or chat.', clusters: true });
  if (state === 'image') notes.push({ text: "This local cluster can't list its agents yet." });
  if (state === 'read') notes.push({ text: "Couldn't check just now.", seen, check: true });
  if (i.files?.state === 'complete' && !history && !i.filesStale) notes.push({ text: "Test history is unavailable, so other agents on this cluster aren't shown and results are unknown." });
  if (history && state === 'ok' && !local.length && !strays.length) notes.push({ text: 'No agents on this computer yet.' });
  const groups: Group[] = [{ key: 'local', title: 'This computer', sub: 'local cluster', notes, cards: [...local, ...strays], count: local.length + strays.length || undefined }];
  (i.places?.places ?? []).forEach(place => {
    const cards = built.remoteCards.filter(c => c.groupKey === place.key);
    const n: Note[] = [];
    if (place.observedAt === null) n.push({ text: 'Not connected since Bridge started. Connect it to see its agents.', clusters: true });
    else if (place.stale) n.push({ text: "Couldn't check just now.", seen: place.observedAt, check: true, clusters: true });
    else if (!cards.length) n.push({ text: `No agents in ${place.namespace} yet.` });
    groups.push({ key: place.key, title: labels.get(place.key)!, sub: place.namespace, notes: n, cards, count: place.observedAt === null ? undefined : cards.length });
  });
  return groups;
}
export function agentGroups(cards: Card[], files: SavedFile[]): Group[] {
  const groups: Group[] = files.map(f => ({ key: f.documentId, title: fileTitle(f), description: f.summary?.description || f.reason || undefined, notes: [], cards: cards.filter(c => c.agentKey === f.documentId) }));
  const strays = cards.filter(c => !files.some(f => f.documentId === c.agentKey));
  return [...groups, ...(strays.length ? [{ key: 'unlinked', title: 'Agents without a file', notes: [], cards: strays }] : [])].map(g => ({ ...g, count: g.cards.length }));
}
// Each group goes to the shorter stack by card count; order inside a stack stays logical.
export function balance(groups: Group[]): [Group[], Group[]] {
  const stacks: [Group[], Group[]] = [[], []]; const load = [0, 0];
  groups.forEach(g => { const k = load[1] < load[0] ? 1 : 0; stacks[k].push(g); load[k] += 1 + g.cards.length; });
  return stacks;
}
export function summaryLabel(cards: Card[], partial: boolean) {
  const failed = cards.filter(c => c.status === 'failed').length;
  return `${cards.length} ${cards.length === 1 ? 'agent' : 'agents'}${partial ? ' shown' : ''}${failed ? `, ${failed} failed` : ''}`;
}
// Empty means nothing could be hiding: no files, no remembered clusters, no Agents, every read succeeded. A stopped or
// image-limited local cluster still counts as empty; a failed read, a busy list or hidden unowned Agents do not.
export function isEmpty(i: Inputs, built: Built) {
  const { state, history } = built;
  return i.files?.state === 'complete' && !i.filesStale && !!i.places && !i.placeError && !i.places.places.length && !built.cards.length
    && ['ok', 'stopped', 'image'].includes(state) && (history || !i.observed?.agents.length);
}
