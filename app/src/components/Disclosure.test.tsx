import { createRef, useState } from 'react';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Disclosure } from './Disclosure';

afterEach(cleanup);
it('folds behind a named chevron and keeps the expanded state accessible', () => {
  const { container } = render(<Disclosure label="More settings" hint="Optional"><input aria-label="Model" defaultValue="saved" /></Disclosure>);
  const control = screen.getByRole('button', { name: 'More settings Optional' });
  expect(control.getAttribute('aria-expanded')).toBe('false');
  expect(container.querySelector('details')?.open).toBe(false);
  expect(control.querySelector('svg[aria-hidden="true"]')).toBeTruthy();
  fireEvent.click(control);
  expect(control.getAttribute('aria-expanded')).toBe('true');
  expect(container.querySelector('details')?.open).toBe(true);
  fireEvent.change(screen.getByLabelText('Model'), { target: { value: 'draft' } });
  fireEvent.click(control); fireEvent.click(control);
  expect((screen.getByLabelText('Model') as HTMLInputElement).value).toBe('draft');
});
it('starts failed rows open but lets the creator fold them', () => {
  render(<Disclosure label="Failed test" defaultOpen><p>Answer</p></Disclosure>);
  const control = screen.getByRole('button', { name: 'Failed test' });
  expect(control.getAttribute('aria-expanded')).toBe('true');
  fireEvent.click(control);
  expect(control.getAttribute('aria-expanded')).toBe('false');
});
it('requests controlled changes once without replaying side effects for prop updates', async () => {
  const changed = vi.fn();
  function Controlled({ reset = false }: { reset?: boolean }) {
    const [open, setOpen] = useState(false);
    return <Disclosure label="Help" open={reset ? false : open} onOpenChange={next => { changed(next); setOpen(next); }}>Instructions</Disclosure>;
  }
  const { rerender } = render(<Controlled />);
  fireEvent.click(screen.getByRole('button', { name: 'Help' }));
  expect(screen.getByRole('button', { name: 'Help' }).getAttribute('aria-expanded')).toBe('true');
  await new Promise(resolve => setTimeout(resolve, 10));
  expect(changed.mock.calls).toEqual([[true]]);
  rerender(<Controlled reset />);
  await new Promise(resolve => setTimeout(resolve, 10));
  expect(screen.getByRole('button', { name: 'Help' }).getAttribute('aria-expanded')).toBe('false');
  expect(changed.mock.calls).toEqual([[true]]);
});
it('observes native toggles, retains caller callbacks and provides the summary focus fallback', async () => {
  const toggled = vi.fn(); const changed = vi.fn(); const ref = createRef<HTMLElement>();
  const { container } = render(<Disclosure label="Saved details" summaryRef={ref} onToggle={toggled} onOpenChange={changed}>Saved answer</Disclosure>);
  const details = container.querySelector('details')!;
  details.open = true; fireEvent(details, new Event('toggle'));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Saved details' }).getAttribute('aria-expanded')).toBe('true'));
  expect(changed).toHaveBeenCalledWith(true);
  expect(toggled).toHaveBeenCalled();
  ref.current?.focus();
  expect(document.activeElement).toBe(ref.current);
});
it('keeps nested folds independent', () => {
  render(<Disclosure label="Saved details" defaultOpen><Disclosure label="Identity">hash</Disclosure></Disclosure>);
  fireEvent.click(screen.getByRole('button', { name: 'Identity' }));
  expect(screen.getByRole('button', { name: 'Identity' }).getAttribute('aria-expanded')).toBe('true');
  expect(screen.getByRole('button', { name: 'Saved details' }).getAttribute('aria-expanded')).toBe('true');
});
