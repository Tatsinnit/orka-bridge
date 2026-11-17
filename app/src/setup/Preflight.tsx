import { Disclosure } from '../components/Disclosure';
import { Button } from '../components/Button';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { readPreflight, type HostPreflight } from './preflight-state';
import { formatRequirementGB } from './requirements';
import { StatusIcon } from '../components/DesignSystem';

export default function Preflight({ requiredMemoryBytes, onBlocked, onCluster }: { requiredMemoryBytes: number; onBlocked: (blocked: boolean) => void; onCluster: () => void }) {
  const [result, setResult] = useState<HostPreflight | null>();
  const [checking, setChecking] = useState(true);
  const [attempt, setAttempt] = useState(0);
  const [memoryHelp, setMemoryHelp] = useState(false);
  const [virtualHelp, setVirtualHelp] = useState(false);
  const [notice, setNotice] = useState('');
  const reason = useRef<HTMLParagraphElement>(null);
  const virtualization = result?.virtualization ?? 'unknown';
  const memory = result?.memory ?? 'unknown';
  const failedVirtualization = !['ready', 'unknown'].includes(virtualization);
  const uncertain = !checking && (virtualization === 'unknown' || memory === 'unknown');
  const windowsFix = result?.platform === 'windows' && (virtualization === 'disabled' || virtualization === 'hypervisorUnavailable');
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
  async function help(next: boolean) {
    setVirtualHelp(next);
    if (!next) return;
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
  return <section aria-label="Computer requirements" aria-busy={checking} className={`workspace-preflight${!checking && (failedVirtualization || memory === 'insufficient') ? ' workspace-failure-explanation' : ''}`}>
    <div aria-live="polite">
      {!checking && failedVirtualization && <section aria-label="Virtualization">
        <p className="preflight-result" role="alert"><StatusIcon state="failed" /><span><b>Virtualization: </b><span>{virtualization === 'disabled' ? 'Turned off or unavailable on this PC' : virtualization === 'hypervisorUnavailable' ? 'Windows Hypervisor Platform is unavailable' : 'Not supported on this Mac'}</span></span></p>
        <Disclosure label={windowsFix ? 'Show me how' : 'About this check'} open={virtualHelp} controls="preflight-virtual-help" onOpenChange={next => { void help(next); }}>
        {virtualHelp && <p id="preflight-virtual-help">{result?.platform === 'macos'
          ? 'Bridge needs Apple-silicon virtualization support. If this Mac is itself a virtual machine, its host may not provide it. Use a supported Mac or a cluster instead.'
          : virtualization === 'disabled' ? 'Check that your PC supports virtualization, then enable Intel VT-x or AMD-V in its startup settings and restart.'
          : 'Turn on Windows Hypervisor Platform in Windows features, then restart this PC.'}</p>}</Disclosure>
      </section>}
      {!checking && memory === 'insufficient' && result?.installedMemoryBytes != null && <section aria-label="Installed memory">
        <p className="preflight-result" role="alert"><StatusIcon state="failed" /><span><b>Installed memory: </b><span>{`about ${formatRequirementGB(result.installedMemoryBytes)} installed; about ${formatRequirementGB(requiredMemoryBytes)} needed. Not enough installed memory. See About this check for exact amounts.`}</span></span></p>
        <Disclosure label="About this check" open={memoryHelp} controls="preflight-memory-help" onOpenChange={setMemoryHelp}>
        {memoryHelp && <div id="preflight-memory-help"><p>This is installed RAM, not currently free memory. Closing apps does not increase installed memory. Use a computer with enough installed memory (about {formatRequirementGB(requiredMemoryBytes)}), or choose a cluster instead. An unreadable check is left to local cluster startup; no memory is reserved by this check.</p>
          <p>{`Required: ${requiredMemoryBytes.toLocaleString()} bytes.${result?.installedMemoryBytes != null ? ` Installed: ${result.installedMemoryBytes.toLocaleString()} bytes.` : ''}`}</p></div>}</Disclosure>
      </section>}
    </div>
    <p className={!checking && !failedVirtualization && memory === 'ready' && !uncertain && attempt === 0 ? 'sr-only' : 'muted'} id="preflight-reason" role="status" ref={reason} tabIndex={-1}>{checking ? 'Checking this computer before setup.' : failedVirtualization || memory === 'insufficient' ? 'Fix the computer requirements above before continuing.' : uncertain ? "Bridge couldn't check this computer yet. It checks again when the workspace starts." : 'These checks do not change this computer.'}</p>
    {uncertain && (failedVirtualization || memory === 'insufficient') && <p className="muted">Bridge couldn't check this computer yet. It checks again when the workspace starts.</p>}
    {!checking && (failedVirtualization || memory === 'insufficient') && <div className="workspace-step-actions">
      <Button type="button" kind="secondary" onClick={onCluster}>Use a cluster instead</Button>
      <Button type="button" kind="secondary" onClick={() => { reason.current?.focus(); onBlocked(true); setChecking(true); setNotice(''); setAttempt(value => value + 1); }}>Check again</Button>
      <Button type="button" kind="quiet" onClick={() => { void copy(); }}>Copy details for help</Button>
    </div>}
    {notice && <p role="status">{notice}</p>}
  </section>;
}
