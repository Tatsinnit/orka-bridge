import { type ComponentProps, type ReactNode, type Ref, useState } from 'react';
import './disclosure.css';

type Props = Omit<ComponentProps<'details'>, 'children' | 'defaultOpen'> & {
  label: ReactNode; hint?: string; children: ReactNode; defaultOpen?: boolean;
  summaryRef?: Ref<HTMLElement>; onOpenChange?: (open: boolean) => void; controls?: string;
};
export function Disclosure({ label, hint, children, defaultOpen = false, open, onOpenChange, onToggle, summaryRef, controls, className = '', ...props }: Props) {
  const [internalOpen, setInternalOpen] = useState(defaultOpen);
  const expanded = open ?? internalOpen;
  function change(next: boolean) {
    if (open === undefined) setInternalOpen(next);
    onOpenChange?.(next);
  }
  return <details {...props} className={`disclosure ${className}`} open={expanded} onToggle={event => {
    // Prop-driven toggles are observations, not another request to open external help.
    if (event.currentTarget.open !== expanded) change(event.currentTarget.open);
    onToggle?.(event);
  }}>
    <summary ref={summaryRef} role="button" tabIndex={0} aria-expanded={expanded} aria-controls={controls} onClick={event => { event.preventDefault(); change(!expanded); }} onKeyDown={event => {
      if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); event.currentTarget.click(); }
    }}>
      <svg className="icon disclosure-chevron" viewBox="0 0 24 24" aria-hidden="true"><path d="m9 6 6 6-6 6" /></svg>
      <span className="disclosure-label">{label}</span>{hint && <> <span className="disclosure-hint">{hint}</span></>}
    </summary>
    {children}
  </details>;
}
