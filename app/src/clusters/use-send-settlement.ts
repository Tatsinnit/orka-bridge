import { useCallback, useEffect, useRef } from 'react';
import { SAMPLE_DOCUMENT_ID } from '../designer/agent-api';
import { sendCall, type SendStatus } from './send-api';

export type SendStart = (args: Record<string, unknown>) => Promise<SendStatus>;
type Attempt = { documentId: string; id: string };

/** App owns admission observation; the editor may disappear before even the start reply. */
export function useSendSettlement(onSettled: () => void): SendStart {
  const lifetime = useRef(0);
  const attempts = useRef(new Map<string, Attempt>());
  const settled = useRef(new Set<string>());
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const reading = useRef(false);
  useEffect(() => () => {
    lifetime.current++;
    clearTimeout(timer.current);
    timer.current = undefined;
    attempts.current.clear();
    settled.current.clear();
  }, []);
  const settle = useCallback((owned: Attempt) => {
    const key = `${owned.documentId}/${owned.id}`;
    if (attempts.current.get(key) !== owned) return;
    attempts.current.delete(key);
    settled.current.add(key);
    if (!attempts.current.size) {
      clearTimeout(timer.current);
      timer.current = undefined;
    }
    onSettled();
  }, [onSettled]);
  const observe = useCallback(async function observe() {
    timer.current = undefined;
    if (!attempts.current.size || reading.current) return;
    const generation = lifetime.current;
    reading.current = true;
    try {
      // Snapshot this round: later admissions retain their own observation, never this read's verdict.
      for (const [key, owned] of [...attempts.current]) {
        if (generation !== lifetime.current) return;
        if (attempts.current.get(key) !== owned) continue;
        try {
          const result = await sendCall('send_status', { documentId: owned.documentId });
          if (generation !== lifetime.current) return;
          // A replacement invalidates only this identity, without adopting the replacement verdict.
          // Null/unreadable status is not evidence that the owned attempt finished.
          if (result.view && (result.view.id !== owned.id || result.view.state !== 'running')) settle(owned);
        } catch { /* Unknown read evidence retries observation only, never submission. */ }
      }
    } finally {
      reading.current = false;
      if (generation === lifetime.current && attempts.current.size) {
        timer.current = setTimeout(() => { void observe(); }, 500);
      }
    }
  }, [settle]);
  return useCallback(async args => {
    const generation = lifetime.current;
    const documentId = args.documentId ?? SAMPLE_DOCUMENT_ID;
    const result = await sendCall('send_start', args);
    const view = result.view;
    if (generation !== lifetime.current || !view || view.documentId !== documentId
      || result.busy && result.busy.documentId !== documentId) return result;
    const key = `${view.documentId}/${view.id}`;
    if (settled.current.has(key)) return result;
    // Native awaits document lookup before admission, so UI request/reply order cannot choose ownership.
    let owned = attempts.current.get(key);
    if (!owned) {
      owned = { documentId: view.documentId, id: view.id };
      attempts.current.set(key, owned);
    }
    if (view.state !== 'running') settle(owned);
    else if (!reading.current && timer.current === undefined) {
      timer.current = setTimeout(() => { void observe(); }, 500);
    }
    return result;
  }, [observe, settle]);
}
