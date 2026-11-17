import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { RelativeTime, relativeTime } from './RelativeTime';

const now = Date.UTC(2026, 10, 13, 12);
afterEach(() => { cleanup(); vi.useRealTimers(); });

test.each([
  [0, 'Just now'], [59_000, 'Just now'], [60_000, '1 minute ago'],
  [3_600_000, '1 hour ago'], [86_400_000, '1 day ago'],
  [172_800_000, '2 days ago'], [-120_000, 'in 2 minutes'],
])('formats a timestamp %s milliseconds before now as %s', (age, expected) => {
  expect(relativeTime(now - age, now)).toBe(expected);
});

test.each([NaN, Infinity, -Infinity, 9e15])('unreadable timestamp %s does not invent an age', value => {
  expect(relativeTime(value, now)).toBe('Time unknown');
});

test('keyboard users can focus a timestamp with an exact-date text equivalent', async () => {
  const value = Date.UTC(2026, 10, 13, 11, 58);
  const exact = new Date(value).toLocaleString();
  const { container } = render(<RelativeTime value={value} />);
  const time = container.querySelector('time')!;
  await userEvent.setup().tab();
  expect(document.activeElement).toBe(time);
  expect(time.getAttribute('aria-label')).toContain(exact);
  expect(time.querySelector('.control-tip')?.textContent).toBe(exact);
});

test('milliseconds from Overview retain an exact machine-readable and hover date', () => {
  vi.useFakeTimers(); vi.setSystemTime(now);
  const timestamp = Date.UTC(2026, 10, 13, 11, 58);
  render(<RelativeTime value={timestamp} />);
  const time = screen.getByText('2 minutes ago');
  expect(time.tagName).toBe('TIME');
  expect(time.getAttribute('datetime')).toBe('2026-11-13T11:58:00.000Z');
  expect(time.title).toBe(new Date(timestamp).toLocaleString());
});
