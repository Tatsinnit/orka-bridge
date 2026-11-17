const formatter = new Intl.RelativeTimeFormat('en', { numeric: 'always' });

export function relativeTime(value: number, now = Date.now()): string {
  if (!Number.isFinite(value) || !Number.isFinite(new Date(value).getTime())) return 'Time unknown';
  const elapsed = now - value;
  if (Math.abs(elapsed) < 60_000) return 'Just now';
  const [unit, size] = Math.abs(elapsed) < 3_600_000 ? ['minute', 60_000] as const
    : Math.abs(elapsed) < 86_400_000 ? ['hour', 3_600_000] as const : ['day', 86_400_000] as const;
  return formatter.format(-Math.trunc(elapsed / size), unit);
}

// Ages refresh with the existing view; the exact timestamp is always available on hover.
export function RelativeTime({ value }: { value: number }) {
  const date = new Date(value);
  const exact = date.toLocaleString(), age = relativeTime(value);
  return Number.isFinite(date.getTime())
    ? <time className="coming-control" tabIndex={0} dateTime={date.toISOString()} title={exact} aria-label={`${age}. ${exact}`}>
      {age}<span className="control-tip" aria-hidden="true">{exact}</span>
    </time>
    : <>Time unknown</>;
}
