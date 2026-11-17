import { createRef } from 'react';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { Button, CloseIcon } from './Button';

afterEach(cleanup);
describe('Button', () => {
  it.each(['primary', 'secondary', 'quiet', 'icon'] as const)('renders the %s kind with native semantics', kind => {
    render(<Button kind={kind} aria-label="Action" className="layout">{kind === 'icon' ? <CloseIcon /> : 'Action'}</Button>);
    const button = screen.getByRole('button', { name: 'Action' });
    expect(button.getAttribute('type')).toBe('button');
    expect(button.classList.contains(`button-${kind}`)).toBe(true);
    expect(button.classList.contains('layout')).toBe(true);
    if (kind === 'icon') expect(button.querySelector('svg')?.getAttribute('aria-hidden')).toBe('true');
  });
  it('forwards refs, native attributes and enabled/disabled clicks', async () => {
    const ref = createRef<HTMLButtonElement>(); const click = vi.fn();
    const view = render(<Button ref={ref} size="small" onClick={click} title="Why" aria-expanded={false}>Action</Button>);
    const button = screen.getByRole('button', { name: 'Action' });
    expect(ref.current).toBe(button);
    ref.current!.focus(); expect(document.activeElement).toBe(button);
    expect(button.classList.contains('button-small')).toBe(true);
    expect(button.getAttribute('title')).toBe('Why');
    expect(button.getAttribute('aria-expanded')).toBe('false');
    await userEvent.click(button); expect(click).toHaveBeenCalledTimes(1);
    view.rerender(<Button disabled onClick={click}>Action</Button>);
    await userEvent.click(screen.getByRole('button', { name: 'Action' }));
    expect(click).toHaveBeenCalledTimes(1);
  });
  it('preserves explicit form submit', async () => {
    const submit = vi.fn(event => event.preventDefault());
    render(<form onSubmit={submit}><Button type="submit">Send</Button></form>);
    await userEvent.click(screen.getByRole('button', { name: 'Send' }));
    expect(submit).toHaveBeenCalledTimes(1);
  });
});
