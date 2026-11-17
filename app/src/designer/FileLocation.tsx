import { Button, CopyIcon } from '../components/Button';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { useState } from 'react';

export default function FileLocation({ path, documentId }: { path?: string; documentId: string }) {
  const [feedback, setFeedback] = useState<{ path?: string; text: string }>();
  const filePath = path;
  async function copy() {
    if (!filePath) return;
    try { await navigator.clipboard.writeText(filePath); setFeedback({ path: filePath, text: 'Path copied' }); }
    catch { setFeedback({ path: filePath, text: 'Could not copy the path' }); }
  }
  async function reveal() {
    try {
      if (!isTauri()) throw Error('Desktop only');
      await invoke('agent_reveal', { documentId });
      setFeedback({ path: filePath, text: 'Folder opened' });
    } catch { setFeedback({ path: filePath, text: 'Could not show the folder. Copy the path to open it yourself.' }); }
  }
  return <div className="file-location">{path ? <code className="agent-path">{path}</code> : <p className="muted">The file path is unavailable.</p>}<div className="agent-tool-row">
    {path && <Button type="button" kind="icon" aria-label="Copy file path" title="Copy file path" onClick={() => { void copy(); }}><CopyIcon /></Button>}
    <Button type="button" kind="secondary" onClick={() => { void reveal(); }}>Show in folder</Button>
    <span className="muted" role="status">{feedback && feedback.path === path ? feedback.text : ''}</span>
  </div></div>;
}
