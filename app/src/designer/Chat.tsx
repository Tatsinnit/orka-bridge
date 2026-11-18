import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { Button } from '../components/Button';
import { StatusIcon } from '../components/DesignSystem';
import { Disclosure } from '../components/Disclosure';
import { type ChatCommand, type ChatSnapshot, chatCall, chatErrors } from './chat-api';

const states: Record<ChatSnapshot['state'], string> = {
  ready: 'Ready for your message.', preparing: 'Preparing your saved agent…', working: 'Your agent is working…',
  waitingForInput: 'Your agent requested input that Chat does not support. You can request Stop.',
  waitingForSession: 'Waiting for the conversation to finish saving…',
  observationLost: 'The connection was lost. Reconnect to check what happened; do not resend the message.',
  requestingStop: 'Requesting stop…', stopRequested: 'Stop requested. Execution termination is unconfirmed.',
  stopUnconfirmed: 'Stop request unconfirmed. Execution may still be running.',
  contextUnavailable: 'Earlier messages could not be confirmed. Start a new conversation when available.',
};
export default function Chat({ documentId, blocked, active = true }: { documentId: string; blocked?: string; active?: boolean }) {
  const [snapshot, setSnapshot] = useState<ChatSnapshot>();
  const [draft, setDraft] = useState(''); const draftRef = useRef('');
  const [error, setError] = useState<{ message: string; uncertainSend?: boolean }>(); const [readError, setReadError] = useState<string>();
  const [command, setCommand] = useState<ChatCommand>();
  const mutation = useRef(false); const epoch = useRef(0); const alive = useRef(true);
  const pending = useRef<{ message: string; count: number; scrollTop?: number; uncertain?: boolean; readerMovedAway?: boolean } | undefined>(undefined);
  const [awaitingTurn, setAwaitingTurn] = useState(false);
  const composer = useRef<HTMLTextAreaElement>(null); const composing = useRef(false);
  const transcript = useRef<HTMLElement>(null); const scrollArea = useRef<HTMLElement | null>(null); const follow = useRef(true);
  const resumeFollow = () => {
    const submitted = pending.current; const area = scrollArea.current;
    // Geometry can change before the browser delivers its scroll event.
    if (submitted && area && (submitted.readerMovedAway || area.scrollTop !== submitted.scrollTop)) {
      follow.current = area.scrollHeight - area.clientHeight - area.scrollTop < 48;
    } else if (!submitted?.readerMovedAway) follow.current = true;
  };
  const receive = (value: ChatSnapshot) => {
    const turns = value.versions.flatMap(version => version.turns);
    const submitted = pending.current;
    if (submitted && turns.slice(submitted.count).some(turn => turn.message === submitted.message)) {
      if (submitted.uncertain) resumeFollow();
      if (draftRef.current === submitted.message) { draftRef.current = ''; setDraft(''); }
      pending.current = undefined; setAwaitingTurn(false);
    } else if (submitted) {
      if (value.busy) {
        if (submitted.uncertain) resumeFollow();
        submitted.uncertain = false;
      }
      // A lost IPC reply is not a rejection. An idle snapshot alone may precede
      // admission; retain acknowledgement until a turn or its busy→idle is seen.
      if (!value.busy && (value.state === 'contextUnavailable' || !submitted.uncertain && value.state === 'ready')) {
        pending.current = undefined; setAwaitingTurn(false);
      }
    }
    if (submitted && !pending.current) setError(current => current?.uncertainSend ? undefined : current);
    setSnapshot(value);
  };
  const receiveRef = useRef(receive); receiveRef.current = receive;
  useEffect(() => {
    alive.current = true; let mounted = true; let timer: ReturnType<typeof setTimeout>;
    const observe = async () => {
      if (!mounted) return;
      if (!mutation.current) {
        const request = epoch.current;
        const result = await chatCall('agent_chat_status', documentId);
        if (mounted && request === epoch.current) {
          if (result.ok) { receiveRef.current(result.snapshot); setReadError(undefined); } else setReadError(result.message);
        }
      }
      if (mounted) timer = setTimeout(() => { void observe(); }, 500);
    };
    void observe();
    return () => { mounted = false; alive.current = false; epoch.current++; clearTimeout(timer); };
  }, [documentId]);
  useLayoutEffect(() => {
    if (!active) return;
    const conversation = transcript.current; const dock = composer.current?.closest('form');
    const area = conversation?.closest<HTMLElement>('.page-content');
    if (!area || !conversation || !dock) return;
    scrollArea.current = area;
    let frame: number | undefined;
    const track = () => {
      follow.current = area.scrollHeight - area.clientHeight - area.scrollTop < 48;
      // A pending Send must not override reading begun after submission.
      if (pending.current && !follow.current && area.scrollTop !== pending.current.scrollTop) pending.current.readerMovedAway = true;
    };
    const reveal = () => {
      if (frame !== undefined) cancelAnimationFrame(frame);
      const target = document.activeElement;
      if (!(target instanceof HTMLElement)) return;
      // Native focus scrolling does not know the sticky composer obscures main.
      // Measure after that scroll, including the dock's current draft/status height.
      frame = requestAnimationFrame(() => {
        frame = undefined;
        if (document.activeElement !== target) return;
        const bottom = target.getBoundingClientRect().bottom;
        const visibleBottom = Math.min(area.getBoundingClientRect().bottom, dock.getBoundingClientRect().top);
        if (bottom > visibleBottom) area.scrollTop += bottom - visibleBottom;
      });
    };
    area.addEventListener('scroll', track); conversation.addEventListener('focusin', reveal);
    return () => {
      area.removeEventListener('scroll', track); conversation.removeEventListener('focusin', reveal);
      if (frame !== undefined) cancelAnimationFrame(frame);
      scrollArea.current = null;
    };
  }, [active]);
  useLayoutEffect(() => {
    if (active && snapshot && follow.current && scrollArea.current) scrollArea.current.scrollTop = scrollArea.current.scrollHeight;
  }, [active, snapshot]);
  useEffect(() => {
    if (!active) return;
    const field = composer.current;
    if (!field) return;
    const fit = () => {
      field.style.height = 'auto';
      if (draft) field.style.height = `${field.scrollHeight}px`;
    };
    fit();
    window.addEventListener('resize', fit);
    return () => window.removeEventListener('resize', fit);
  }, [draft, active]);
  const act = async (action: ChatCommand) => {
    if (mutation.current || !snapshot) return;
    // The parent's block covers every conversation reset too; Stop/Reconnect stay available for recovery.
    if (action === 'agent_chat_new' && blocked) return;
    if (action === 'agent_chat_send' && (blocked || !snapshot.canSend || awaitingTurn || !draftRef.current.trim())) return;
    mutation.current = true; const request = ++epoch.current; setCommand(action); setError(undefined);
    if (action === 'agent_chat_send') {
      pending.current = { message: draftRef.current, count: snapshot.versions.reduce((n, version) => n + version.turns.length, 0), scrollTop: scrollArea.current?.scrollTop };
      setAwaitingTurn(true);
    }
    const operation = action === 'agent_chat_reconnect' && pending.current?.uncertain ? 'agent_chat_status' : action;
    const result = await chatCall(operation, documentId, pending.current?.message);
    if (!alive.current || request !== epoch.current) return;
    if (result.ok) {
      if (action === 'agent_chat_send') resumeFollow();
      receiveRef.current(result.snapshot);
    } else {
      if (action === 'agent_chat_send') {
        if (result.uncertain && pending.current) pending.current.uncertain = true;
        else { pending.current = undefined; setAwaitingTurn(false); }
      }
      setError({ message: result.message, uncertainSend: action === 'agent_chat_send' && result.uncertain });
    }
    mutation.current = false; setCommand(undefined); composer.current?.focus();
  };
  const reason = blocked ?? (command ? 'Wait for this action to finish.' : awaitingTurn ? 'Wait for this message to be accepted.' : !snapshot ? 'Opening your conversation…' : !snapshot.canSend ? 'Wait for the current conversation or test to finish.' : undefined);
  const latestTurn = snapshot?.versions.at(-1)?.turns.at(-1);
  const status = pending.current?.uncertain ? 'Checking whether your message was accepted. Do not resend it.'
    : command === 'agent_chat_stop' ? states.requestingStop
    : snapshot?.state === 'working' && latestTurn && ['', 'Pending', 'Queued', 'Scheduled'].includes(latestTurn.phase) ? 'Waiting to start…'
    : blocked && snapshot?.state === 'ready' ? 'Chat is unavailable.'
    : blocked && snapshot?.state === 'contextUnavailable' ? 'Earlier messages could not be confirmed.'
    : snapshot ? states[snapshot.state] : 'Opening your conversation…';
  const canStop = snapshot && snapshot.state !== 'ready' && snapshot.state !== 'contextUnavailable';
  const canReset = snapshot && !blocked && !snapshot.busy && !awaitingTurn && ['ready', 'contextUnavailable'].includes(snapshot.state);
  const snapshotError = snapshot?.state === 'contextUnavailable' && snapshot.error
    ? blocked ? `New conversation is unavailable: ${blocked}` : 'Use New conversation to continue without unconfirmed history.'
    : snapshot?.state === 'ready' && snapshot.error === 'connection'
    ? 'The workspace could not be reached. Check Clusters, then try sending again.'
    : snapshot?.error ? chatErrors[snapshot.error] : undefined;
  const hasTurns = snapshot?.versions.some(version => version.turns.length);
  const showControls = hasTurns || blocked || error || readError || snapshotError || awaitingTurn || command || snapshot && snapshot.state !== 'ready';
  return <section className="chat-panel" aria-label="Chat on this computer">
    <h2 className="sr-only">Chat on this computer</h2>
    {showControls && <div className="chat-heading"><Button type="button" kind="quiet" disabled={!canReset || !!command} title={blocked ?? (!canReset ? 'Wait for the current turn to finish or reconnect to check it.' : undefined)} onClick={() => { void act('agent_chat_new'); }}>New conversation</Button>
      <Disclosure className="chat-about" label="About this conversation">
        <p>Your agent remembers up to 50 messages. Interrupted replies may not be remembered; tool activity is shown separately.</p>
        <p>This view clears when you quit Bridge. Agents and transcripts remain in the workspace; New conversation does not delete them.</p>
        <p>Stop requests an end to the reply. A tool may still finish while you continue, and its effects are not undone. If the conversation cannot continue, start a new one. You may need to restart the workspace if its history has changed elsewhere.</p>
      </Disclosure>
    </div>}
    <section ref={transcript} className="chat-transcript" aria-label="Conversation">
      {snapshot?.versions.map((version, index) => <section key={index} className="chat-version" aria-label={snapshot.versions.length > 1 ? `Conversation ${index + 1}` : undefined}>
        {snapshot.versions.length > 1 && <h3>Conversation {index + 1}</h3>}
        {index > 0 && <p className="muted">New conversation. Earlier turns are shown for reference, not sent as history.</p>}
        {version.turns.map((turn, i) => {
          // A terminal Task can be published before the worker finishes reading its result/Session.
          const confirming = turn === latestTurn && (snapshot.state === 'waitingForSession'
            || snapshot.state === 'working' && ['Succeeded', 'Failed', 'Cancelled'].includes(turn.phase));
          const waiting = turn.answer === null && (confirming
            || turn === latestTurn && ['preparing', 'working'].includes(snapshot.state) && ['', 'Pending', 'Queued', 'Scheduled', 'Running', 'Finalizing'].includes(turn.phase));
          return <article className="chat-turn" key={i}>
            <div className="chat-question"><h4>You</h4><p className="reading">{turn.message}</p></div>
            <div className="chat-response"><h4>Agent</h4>
              {turn.phase === 'Failed' && <p>This turn failed.</p>}{turn.phase === 'Cancelled' && <p>The cluster reported this turn cancelled; worker termination is unconfirmed.</p>}
              {turn.answer !== null ? <p className="reading">{turn.answer}</p> : waiting
                ? <p className="chat-waiting"><StatusIcon state="running" />{confirming ? snapshot.state === 'waitingForSession' ? states.waitingForSession : 'Confirming your agent’s reply…' : ['Running', 'Finalizing'].includes(turn.phase) ? 'Waiting for your agent’s reply…' : 'Waiting for your agent to start…'}</p>
                : <p className="muted">{turn.phase === 'Failed' ? 'The turn failed without an answer.' : 'No answer was received.'}</p>}
              {turn.stopUnconfirmed ? <p className="chat-notice">{states.stopUnconfirmed}</p> : turn.terminationUnconfirmed && <p className="chat-notice">{states.stopRequested}</p>}
              {!turn.contextRetained && !waiting && !confirming && <p className="muted">This turn is not confirmed in later context.</p>}
              {!!turn.tools.length && <ul className="chat-tools" aria-label="Tool activity">{turn.tools.slice(-1).map((tool, t) => <li key={t}><code>{tool.name}</code> · {tool.phase}</li>)}</ul>}
              {turn.tools.length > 1 && <Disclosure label={`${turn.tools.length - 1} more tool activit${turn.tools.length === 2 ? 'y' : 'ies'}`}>
                <ul className="chat-tools">{turn.tools.slice(0, -1).map((tool, t) => <li key={t}><code>{tool.name}</code> · {tool.phase}</li>)}</ul>
              </Disclosure>}
            </div>
          </article>;
        })}
      </section>)}
    </section>
    <form className="chat-composer" onSubmit={event => { event.preventDefault(); void act('agent_chat_send'); }}>
      <p className={`chat-status${status === states.ready ? ' sr-only' : ''}`} role="status" aria-live="polite" aria-atomic="true">{status === states.ready && !latestTurn ? undefined : status}</p>
      {(error || readError || snapshotError) && <p role="alert">{error?.message ?? readError ?? snapshotError}</p>}
      <label className="sr-only" htmlFor={`message-${documentId}`}>Message</label>
      <div className="chat-compose-row"><textarea className="field" ref={composer} id={`message-${documentId}`} rows={1} placeholder="Message your agent" value={draft} aria-describedby={`chat-hint-${documentId}`}
        onChange={event => { draftRef.current = event.target.value; setDraft(event.target.value); }}
        onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }}
        onKeyDown={event => {
          if (event.key === 'Enter' && !event.shiftKey && !composing.current && !event.nativeEvent.isComposing && event.keyCode !== 229) {
            event.preventDefault(); void act('agent_chat_send');
          }
        }} />
        <Button type="submit" kind="primary" aria-label="Send message" disabled={!!reason || !draft.trim()} title={reason ?? (!draft.trim() ? 'Write a message first.' : undefined)}>Send</Button>
      </div>
      <p className="muted" id={`chat-hint-${documentId}`}>{reason ?? 'Enter sends · Shift+Enter for a new line'}</p>
      <div className="chat-actions">
        {canStop && <Button type="button" kind="secondary" disabled={!!command || snapshot.state === 'requestingStop'} onClick={() => { void act('agent_chat_stop'); }}>Stop</Button>}
        {snapshot && snapshot.state !== 'contextUnavailable' && !snapshot.busy && (!snapshot.canSend || pending.current?.uncertain) && <Button type="button" kind="secondary" disabled={!!command} onClick={() => { void act('agent_chat_reconnect'); }}>Reconnect</Button>}
      </div>
    </form>
  </section>;
}
