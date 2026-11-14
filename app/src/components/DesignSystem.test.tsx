import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { Details, Elapsed, GeneratedValue, LongCard, StatusPill } from './DesignSystem';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.useRealTimers(); });
describe('shared foundation', () => {
  it.each(['passed', 'failed', 'running', 'stale', 'needs-you', 'not-started'] as const)('names %s without relying on colour', state => {
    const { container } = render(<StatusPill state={state} />);
    expect(screen.getByText({ 'needs-you': 'Needs you', 'not-started': 'Not started', passed: 'Passed', failed: 'Failed', running: 'Running', stale: 'Stale' }[state])).toBeTruthy();
    expect(container.querySelector('svg[aria-hidden="true"]')).toBeTruthy();
  });
  it('labels elapsed observation time without claiming a run start time', () => {
    vi.useFakeTimers();
    const { unmount } = render(<Elapsed />);
    expect(screen.getByText('Observed for 0s')).toBeTruthy();
    act(() => { vi.advanceTimersByTime(2000); });
    expect(screen.getByText('Observed for 2s')).toBeTruthy();
    unmount(); vi.useRealTimers();
  });
  it('folds machine values and copies the full hash rather than its display form', async () => {
    const copied: string[] = [];
    vi.stubGlobal('navigator', { clipboard: { writeText: async (value: string) => { copied.push(value); } } });
    const { container } = render(<Details><GeneratedValue label="Agent hash" value="sha256:7c17abcdefa370" /></Details>);
    expect(container.querySelector('details')?.open).toBe(false);
    fireEvent.click(screen.getByText('Details'));
    expect(container.querySelector('details')?.open).toBe(true);
    expect(screen.getByText('sha256:7c17…a370')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Copy Agent hash' }));
    await waitFor(() => expect(screen.getByRole('status').textContent).toBe('Copied'));
    expect(copied).toEqual(['sha256:7c17abcdefa370']);
  });
  it('does not claim a copy succeeded when the clipboard is unavailable', async () => {
    vi.stubGlobal('navigator', {});
    render(<GeneratedValue label="Task" value="task-1234567890" />);
    fireEvent.click(screen.getByRole('button', { name: 'Copy Task' }));
    await waitFor(() => expect(screen.getByRole('status').textContent).toBe('Could not copy'));
    expect(screen.getByText('task…7890')).toBeTruthy();
  });
  it('keeps short values whole and separates head, summary and named content', () => {
    render(<LongCard title="Local tests" status={<StatusPill state="stale" />} action={<button>Run test</button>} summary="This computer"><Details><GeneratedValue label="Task" value="short" /></Details></LongCard>);
    expect(screen.getByRole('heading', { name: 'Local tests' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Run test' })).toBeTruthy();
    expect(screen.getByText('This computer')).toBeTruthy();
    expect(screen.getByText('short')).toBeTruthy();
  });
});
