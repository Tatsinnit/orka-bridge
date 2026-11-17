import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import AgentParts from './AgentParts';
import { unsupportedToolReason } from './built-in-tools';

const source = '---\nname: reader\nmodel: m\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools: []\n---\nRead.';
beforeEach(() => { Object.defineProperty(HTMLDivElement.prototype, 'showPopover', { configurable: true, value() { this.style.display = 'grid'; } }); });
afterEach(() => { cleanup(); Reflect.deleteProperty(HTMLDivElement.prototype, 'showPopover'); });
test.each(['builtin', 'tool'] as const)('the disabled %s approval reason is readable outside the disabled control and reachable by keyboard', async kind => {
  const changed = vi.fn(); const user = userEvent.setup();
  render(<AgentParts source={source} readOnly={false} onChange={changed} catalog={{ defaultModel: 'm', models: { state: 'ready', items: [{ id: 'm' }] }, tools: { state: 'ready', items: [{ kind, name: 'request_approval', description: 'Ask a person.' }] } }} />);
  await user.click(screen.getByRole('button', { name: 'Add a tool' }));
  const reason = screen.getByText(unsupportedToolReason);
  expect(reason.closest('button')).toBeNull();
  const group = reason.closest<HTMLElement>('[role="group"]')!;
  expect(group).not.toBeNull(); expect(group.tabIndex).toBe(0);
  expect(group.getAttribute('aria-describedby')).toBe(reason.id);
  const button = within(group).getByRole('button') as HTMLButtonElement;
  expect(button.disabled).toBe(true);
  group.focus(); expect(document.activeElement).toBe(group);
  await user.click(button); await user.keyboard('{Enter}');
  expect(changed).not.toHaveBeenCalled();
});
test('a team-cluster approval choice stays enabled without the local refusal', async () => {
  const changed = vi.fn(); const user = userEvent.setup();
  render(<AgentParts source={source} readOnly={false} remote onChange={changed} catalog={{ defaultModel: 'm', models: { state: 'ready', items: [] }, tools: { state: 'ready', items: [{ kind: 'builtin', name: 'request_approval', description: 'Ask a person.' }] } }} />);
  await user.click(screen.getByRole('button', { name: 'Add a tool' }));
  expect(screen.queryByText(unsupportedToolReason)).toBeNull();
  await user.click(screen.getByRole('button', { name: /Ask for approval/ }));
  expect(changed).toHaveBeenCalledOnce(); expect(changed.mock.calls[0][0]).toContain('request_approval');
});
