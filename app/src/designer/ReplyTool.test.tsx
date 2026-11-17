import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import type { CatalogTool, WorkspaceCatalogReply } from '../catalog';
import AgentParts from './AgentParts';
import { readForm } from './agent-form';

const source = '---\nname: reader\nmodel: m\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools: []\n---\nRead.';
const replyReason = 'Conversation replies need an authenticated gateway conversation, which local tests and Chat do not have.';
const catalog = (kind: CatalogTool['kind']): WorkspaceCatalogReply => ({ defaultModel: 'm', models: { state: 'ready', items: [{ id: 'm' }] }, tools: { state: 'ready', items: [{ kind, name: 'reply_in_conversation', description: 'Reply.' }] } });
beforeEach(() => { Object.defineProperty(HTMLDivElement.prototype, 'showPopover', { configurable: true, value() { this.style.display = 'grid'; } }); });
afterEach(() => { cleanup(); Reflect.deleteProperty(HTMLDivElement.prototype, 'showPopover'); });

// Omitting the reply name from the shared local list would allow a silent no-op.
test.each(['builtin', 'tool'] as const)('reply %s stays searchable but disabled with a readable gateway reason', async kind => {
  const changed = vi.fn(); const user = userEvent.setup();
  render(<AgentParts source={source} readOnly={false} catalog={catalog(kind)} onChange={changed} />);
  await user.click(screen.getByRole('button', { name: 'Add a tool' }));
  await user.type(screen.getByRole('searchbox'), 'reply_in_conversation');
  const option = screen.getByRole('button', { name: /reply_in_conversation/ }) as HTMLButtonElement;
  expect(option.disabled).toBe(true);
  const reason = screen.getByText(replyReason);
  expect(reason.closest('button')).toBeNull();
  const group = reason.closest<HTMLElement>('[role="group"]')!;
  expect(group.tabIndex).toBe(0);
  expect(group.getAttribute('aria-describedby')).toBe(reason.id);
  group.focus(); expect(document.activeElement).toBe(group);
  await user.click(option); await user.keyboard('{Enter}');
  expect(changed).not.toHaveBeenCalled();
});

test('a saved reply is warned about but preserved until explicit removal', async () => {
  const saved = source.replace('tools: []', 'tools: [{kind: builtin, name: reply_in_conversation}]');
  const changed = vi.fn(); const user = userEvent.setup();
  const view = render(<AgentParts source={saved} readOnly={false} onChange={changed} />);
  expect(screen.getByText('Reply in a conversation')).toBeTruthy();
  expect(screen.getByRole('alert').textContent).toMatch(/Remove all .*reply_in_conversation tools before running local tests or Chat/);
  expect(screen.getByRole('alert').textContent).not.toContain('No work was submitted');
  expect(changed).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Remove Reply in a conversation' }));
  const edited = changed.mock.calls[0][0];
  expect(readForm(edited).tools).toEqual([]);
  view.rerender(<AgentParts source={edited} readOnly={false} onChange={changed} />);
  expect(screen.queryByRole('alert')).toBeNull();
});

test('a saved agent with both unsupported tools is warned until both are removed', async () => {
  const saved = source.replace('tools: []', 'tools: [{kind: builtin, name: request_approval}, {kind: builtin, name: reply_in_conversation}]');
  const changed = vi.fn(); const user = userEvent.setup();
  const view = render(<AgentParts source={saved} readOnly={false} onChange={changed} />);
  expect(screen.getByRole('alert').textContent).toBe('Remove all request_approval and reply_in_conversation tools before running local tests or Chat.');
  await user.click(screen.getByRole('button', { name: 'Remove Reply in a conversation' }));
  const approvalOnly = changed.mock.calls.at(-1)![0];
  view.rerender(<AgentParts source={approvalOnly} readOnly={false} onChange={changed} />);
  expect(screen.getByRole('alert')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Remove Ask for approval' }));
  view.rerender(<AgentParts source={changed.mock.calls.at(-1)![0]} readOnly={false} onChange={changed} />);
  expect(screen.queryByRole('alert')).toBeNull();
});

test('each unavailable choice explains its own requirement when both are listed', async () => {
  const reply = catalog('builtin');
  render(<AgentParts source={source} readOnly={false} catalog={{ ...reply, tools: { state: 'ready', items: [{ kind: 'builtin', name: 'request_approval', description: 'Approve.' }, { kind: 'builtin', name: 'reply_in_conversation', description: 'Reply.' }] } }} onChange={() => {}} />);
  await userEvent.setup().click(screen.getByRole('button', { name: 'Add a tool' }));
  const approval = screen.getByRole('group', { name: 'Ask for approval' });
  expect(within(approval).getByText(/Approval tools need autonomous coordination/)).toBeTruthy();
  expect(within(approval).queryByText(replyReason)).toBeNull();
  expect(within(screen.getByRole('group', { name: 'Reply in a conversation' })).getByText(replyReason)).toBeTruthy();
});

test.each(['builtin', 'tool'] as const)('a remote reply %s choice remains addable and saved without local warnings', async kind => {
  const changed = vi.fn(); const user = userEvent.setup();
  const view = render(<AgentParts source={source} remote readOnly={false} catalog={catalog(kind)} onChange={changed} />);
  await user.click(screen.getByRole('button', { name: 'Add a tool' }));
  const option = screen.getByRole('button', { name: /reply_in_conversation/ }) as HTMLButtonElement;
  expect(option.disabled).toBe(false);
  expect(screen.queryByText(replyReason)).toBeNull();
  await user.click(option);
  const edited = changed.mock.calls[0][0];
  expect(readForm(edited).tools[0]).toMatchObject({ name: 'reply_in_conversation', kind });
  view.rerender(<AgentParts source={edited} remote readOnly={false} onChange={changed} />);
  expect(screen.queryByRole('alert')).toBeNull();
});
