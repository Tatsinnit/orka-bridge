import { Disclosure } from '../components/Disclosure';
import { Button } from '../components/Button';
import { useEffect, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { readPreflight, type HostPreflight } from './preflight-state';
import { formatRequirementGB } from './requirements';
import { StatusIcon } from '../components/DesignSystem';

export default function Preflight({ requiredMemoryBytes, onBlocked, action }: { requiredMemoryBytes: number; onBlocked: (blocked: boolean) => void; action?: ReactNode }) {
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
  return <section aria-label="Computer requirements" aria-busy={checking} className="workspace-preflight">
    <div aria-live="polite">
      <section aria-label="Virtualization">
        <p className="preflight-result" role={failedVirtualization && !checking ? 'alert' : undefined}><StatusIcon state={checking ? 'running' : failedVirtualization ? 'failed' : virtualization === 'ready' ? 'passed' : 'not-started'} /><span><b>Virtualization: </b><span>{checking ? 'Checking…' : {
          ready: 'Available', disabled: 'Turned off or unavailable on this PC', hypervisorUnavailable: 'Windows Hypervisor Platform is unavailable', unsupported: 'Not supported on this Mac', unknown: 'Not checked',
        }[virtualization]}</span></span></p>
        <Disclosure label={windowsFix ? 'Show me how' : 'About this check'} open={virtualHelp} controls="preflight-virtual-help" onOpenChange={next => { void help(next); }}>
        {virtualHelp && <p id="preflight-virtual-help">{result?.platform === 'macos'
          ? 'Bridge needs Apple-silicon virtualization support. If this Mac is itself a virtual machine, its host may not provide it. Use a supported Mac or a cluster instead.'
          : virtualization === 'disabled' ? 'Check that your PC supports virtualization, then enable Intel VT-x or AMD-V in its startup settings and restart.'
          : virtualization === 'hypervisorUnavailable' ? 'Turn on Windows Hypervisor Platform in Windows features, then restart this PC.'
          : "If Bridge couldn't check, starting the local cluster will check virtualization. You can also use a cluster instead."}</p>}</Disclosure>
      </section>
      <section aria-label="Installed memory">
        <p className="preflight-result" role={memory === 'insufficient' && !checking ? 'alert' : undefined}><StatusIcon state={checking ? 'running' : memory === 'insufficient' ? 'failed' : memory === 'ready' ? 'passed' : 'not-started'} /><span><b>Installed memory: </b><span>{checking ? 'Checking…' : result?.installedMemoryBytes != null
          ? `about ${formatRequirementGB(result.installedMemoryBytes)} installed; about ${formatRequirementGB(requiredMemoryBytes)} needed${memory === 'insufficient' ? '. Not enough installed memory. See About this check for exact amounts.' : '.'}`
          : 'Not checked'}</span></span></p>
        <Disclosure label="About this check" open={memoryHelp} controls="preflight-memory-help" onOpenChange={setMemoryHelp}>
        {memoryHelp && <div id="preflight-memory-help"><p>This is installed RAM, not currently free memory. Closing apps does not increase installed memory. Use a computer with enough installed memory (about {formatRequirementGB(requiredMemoryBytes)}), or choose a cluster instead. An unreadable check is left to local cluster startup; no memory is reserved by this check.</p>
          <p>{`Required: ${requiredMemoryBytes.toLocaleString()} bytes.${result?.installedMemoryBytes != null ? ` Installed: ${result.installedMemoryBytes.toLocaleString()} bytes.` : ''}`}</p></div>}</Disclosure>
      </section>
    </div>
    <p className="muted" id="preflight-reason" role="status">{checking ? 'Checking this computer before setup.' : failedVirtualization || memory === 'insufficient' ? 'Fix the computer requirements above before continuing.' : uncertain ? 'Bridge checks this when it starts.' : 'These checks do not change this computer.'}</p>
    <div className="workspace-step-actions">
      {action}
      <Button type="button" kind="secondary" disabled={checking} onClick={() => { onBlocked(true); setChecking(true); setNotice(''); setAttempt(value => value + 1); }}>Check again</Button>
      <Button type="button" kind="quiet" disabled={checking} onClick={() => { void copy(); }}>Copy details for help</Button>
    </div>
    {notice && <p role="status">{notice}</p>}
  </section>;
}
