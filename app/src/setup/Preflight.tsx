import { Button } from '../components/Button';
import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { readPreflight, type HostPreflight } from './preflight-state';
import { formatSize } from './requirements';
import { StatusIcon } from '../components/DesignSystem';

export default function Preflight({ requiredMemoryBytes, onBlocked, onUncertain }: { requiredMemoryBytes: number; onBlocked: (blocked: boolean) => void; onUncertain?: (uncertain: boolean) => void }) {
  const [result, setResult] = useState<HostPreflight | null>();
  const [checking, setChecking] = useState(true);
  const [attempt, setAttempt] = useState(0);
  const [memoryHelp, setMemoryHelp] = useState(false);
  const [virtualHelp, setVirtualHelp] = useState(false);
  const [notice, setNotice] = useState('');
  const virtualization = result?.virtualization ?? 'unknown';
  const memory = result?.memory ?? 'unknown';
  const failedVirtualization = !['ready', 'unknown'].includes(virtualization);
  const uncertain = !checking && (virtualization === 'unknown' || memory === 'unknown');
  useEffect(() => { onUncertain?.(uncertain); }, [uncertain, onUncertain]);
  useEffect(() => {
    let active = true;
    onBlocked(true);
    void readPreflight(requiredMemoryBytes).then(value => {
      if (!active) return;
      setResult(value); setChecking(false);
      onBlocked(!!value && (value.memory === 'insufficient' || !['ready', 'unknown'].includes(value.virtualization)));
    });
    return () => { active = false; onBlocked(false); };
  }, [attempt, requiredMemoryBytes, onBlocked]);
  async function help() {
    if (virtualHelp) { setVirtualHelp(false); return; }
    setVirtualHelp(true);
    const topic = virtualization === 'hypervisorUnavailable' ? 'hypervisor' : virtualization === 'disabled' ? 'virtualization' : null;
    if (result?.platform !== 'windows' || !topic) return;
    try { await invoke('setup_open_help', { topic }); setNotice('Help opened in your browser.'); }
    catch { setNotice("Couldn't open help. Check your PC manufacturer's virtualization instructions, or use a cluster instead."); }
  }
  async function copy() {
    const details = ['Bridge computer requirements', `Platform: ${result?.platform ?? 'unknown'}`, `Virtualization: ${virtualization}`, `Installed memory check: ${memory}`, 'Private diagnostic text is not included.'].join('\n');
    try { await navigator.clipboard.writeText(details); setNotice('Details copied.'); }
    catch { setNotice("Couldn't copy details. The check results are shown above."); }
  }
  return <section aria-label="Computer requirements" aria-busy={checking} className="workspace-preflight">
    <div aria-live="polite">
      <section aria-label="Virtualization">
        <p role={failedVirtualization && !checking ? 'alert' : undefined}><StatusIcon state={checking ? 'running' : failedVirtualization ? 'failed' : virtualization === 'ready' ? 'passed' : 'not-started'} /><b>Virtualization: </b>{checking ? 'Checking…' : {
          ready: 'Available', disabled: 'Turned off or unavailable on this PC', hypervisorUnavailable: 'Windows Hypervisor Platform is unavailable', unsupported: 'Not supported on this Mac', unknown: "Couldn't check",
        }[virtualization]}</p>
        <button type="button" className="workspace-disclosure" aria-expanded={virtualHelp} aria-controls="preflight-virtual-help" onClick={() => { void help(); }}>Show me how</button>
        {virtualHelp && <p id="preflight-virtual-help">{result?.platform === 'macos'
          ? 'Bridge needs Apple-silicon virtualization support. If this Mac is itself a virtual machine, its host may not provide it. Use a supported Mac or a cluster instead.'
          : virtualization === 'disabled' ? 'Check that your PC supports virtualization, then enable Intel VT-x or AMD-V in its startup settings and restart.'
          : virtualization === 'hypervisorUnavailable' ? 'Turn on Windows Hypervisor Platform in Windows features, then restart this PC.'
          : "If Bridge couldn't check, starting the local cluster will check virtualization. You can also use a cluster instead."}</p>}
      </section>
      <section aria-label="Installed memory">
        <p role={memory === 'insufficient' && !checking ? 'alert' : undefined}><StatusIcon state={checking ? 'running' : memory === 'insufficient' ? 'failed' : memory === 'ready' ? 'passed' : 'not-started'} /><b>Installed memory: </b>{checking ? 'Checking…' : result?.installedMemoryBytes != null
          ? `${formatSize(result.installedMemoryBytes)} installed; ${formatSize(requiredMemoryBytes)} needed${memory === 'insufficient' ? '. Not enough installed memory.' : '.'}`
          : "Couldn't check"}</p>
        <button type="button" className="workspace-disclosure" aria-expanded={memoryHelp} aria-controls="preflight-memory-help" onClick={() => setMemoryHelp(value => !value)}>Show me how</button>
        {memoryHelp && <p id="preflight-memory-help">This is installed RAM, not currently free memory. Closing apps does not increase installed memory. Use a computer with at least {formatSize(requiredMemoryBytes)} installed, or choose a cluster below. An unreadable check is left to local cluster startup; no memory is reserved by this check.</p>}
      </section>
    </div>
    <p className="muted" id="preflight-reason" role="status">{checking ? 'Checking this computer before setup.' : failedVirtualization || memory === 'insufficient' ? 'Fix the computer requirements above before continuing.' : uncertain ? 'Some requirements could not be checked. Check again, or continue to try starting the local cluster.' : 'These checks do not change this computer.'}</p>
    <div className="workspace-step-actions">
      <Button type="button" kind={uncertain ? 'primary' : 'secondary'} disabled={checking} onClick={() => { onBlocked(true); setChecking(true); setNotice(''); setAttempt(value => value + 1); }}>Check again</Button>
      <Button type="button" kind="secondary" disabled={checking} onClick={() => { void copy(); }}>Copy details for help</Button>
    </div>
    {notice && <p role="status">{notice}</p>}
  </section>;
}
