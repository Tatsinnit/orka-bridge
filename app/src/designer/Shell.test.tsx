import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Example from './Example';
import { SAMPLE_DOCUMENT_ID as id } from './agent-api';

const other = '00000000-0000-4000-8000-000000000002';
const pair = { documentId: id, name: 'reader', agent: '---\nname: reader\n---\nRead a page.', tests: '# Checks' };
const idle = { documentId: id, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
function native(extra?: (command: string) => unknown) {
  vi.stubGlobal('isTauri', true);
  mockIPC((command, value) => {
    const result = extra?.(command); if (result !== undefined) return result;
    const args = value as { documentId: string };
    if (command === 'agent_open') return { ...pair, documentId: args.documentId, name: args.documentId === id ? 'reader' : 'helper', agent: args.documentId === id ? pair.agent : pair.agent.replace('name: reader', 'name: helper') };
    if (command === 'agents_list') return [{ id, name: 'reader', path: null, available: true }, { id: other, name: 'helper', path: null, available: true }];
    if (command === 'agent_select') return null;
    if (command === 'agent_creation_root') return '/agents';
    if (command === 'agent_create') return other;
    if (command === 'agent_run_status') return { ...idle, documentId: args.documentId };
    if (command === 'send_status') return { documentId: args.documentId, view: null, busy: null };
    if (command === 'send_history') return [];
    if (command === 'agent_chat_status') return { documentId: args.documentId, state: 'ready', busy: false, canSend: true, versions: [], error: null };
    throw Error(`Unexpected ${command}`);
  });
}

test('explicit same-document Open selects Agent, but a top-level navigation return retains File', async () => {
  native(); const view = render(<Example />); const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'File' }));
  view.rerender(<Example active={false} />); view.rerender(<Example active />);
  expect(screen.getByRole('button', { name: 'File' }).getAttribute('aria-pressed')).toBe('true');
  view.rerender(<Example openDocumentRequest={1} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Agent' }).getAttribute('aria-pressed')).toBe('true'));
  expect(screen.getByRole('textbox', { name: 'Instructions' })).toBeTruthy();
});

test('a rejected explicit Open retains File and its draft', async () => {
  native(command => { if (command === 'agent_select') throw Error('unavailable'); });
  const view = render(<Example />); const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'File' }));
  view.rerender(<Example openDocumentRequest={1} />);
  await screen.findByText('Your agent selection could not be saved. Try choosing it again.');
  expect(screen.getByRole('button', { name: 'File' }).getAttribute('aria-pressed')).toBe('true');
  expect((screen.getByRole('textbox', { name: 'Agent file' }) as HTMLTextAreaElement).value).toBe(pair.agent);
});

test('the agent picker opens its document in Agent rather than the previous File view', async () => {
  native(); render(<Example />); const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'File' }));
  fireEvent.change(await screen.findByRole('combobox', { name: 'Agent' }), { target: { value: other } });
  await screen.findByRole('heading', { name: 'helper', level: 1 });
  expect(screen.getByRole('button', { name: 'Agent' }).getAttribute('aria-pressed')).toBe('true');
});

test('New agent cancellation retains File, while successful creation opens Agent', async () => {
  native(); render(<Example />); const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'File' }));
  await user.click(screen.getByRole('button', { name: 'New agent' }));
  await user.click(await screen.findByRole('button', { name: 'Cancel' }));
  expect(screen.getByRole('button', { name: 'File' }).getAttribute('aria-pressed')).toBe('true');
  await user.click(screen.getByRole('button', { name: 'New agent' }));
  await user.type(await screen.findByRole('textbox', { name: 'Name' }), 'helper');
  await user.click(screen.getByRole('button', { name: 'Create' }));
  await screen.findByRole('heading', { name: 'helper', level: 1 });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Agent' }).getAttribute('aria-pressed')).toBe('true'));
});

test('an unused Send has no card or Team cluster heading; its entry remains on Chat and reveals/focuses it', async () => {
  native(); render(<Example />); const user = userEvent.setup();
  await screen.findByRole('textbox', { name: 'Instructions' });
  expect(screen.queryByRole('region', { name: 'Send and cluster tests' })).toBeNull();
  expect(screen.queryByRole('heading', { name: 'Team cluster' })).toBeNull();
  const history = screen.getByRole('button', { name: 'Past Sends' });
  expect(history.getAttribute('aria-expanded')).toBe('false');
  await user.click(history);
  expect(await screen.findByText('No saved Sends yet.')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Chat' }));
  await user.click(screen.getByRole('button', { name: 'Send to a cluster' }));
  const send = await screen.findByRole('region', { name: 'Send and cluster tests' });
  await waitFor(() => expect(document.activeElement).toBe(send));
  expect(screen.getByRole('button', { name: 'Agent' }).getAttribute('aria-pressed')).toBe('true');
  expect(screen.queryByText('Team clusters are early in this preview.')).toBeNull();
  expect(screen.getByRole('button', { name: 'Choose a cluster' }).className).toContain('primary');
  const action = screen.getByRole('button', { name: 'Send and run tests' });
  expect(action.hasAttribute('disabled')).toBe(true);
  expect(screen.getByText('Choose a cluster and AI connection in Clusters first')).toBeTruthy();
  expect(screen.queryByText('Needs attention')).toBeNull();
});

test('a delayed document list reserves a disabled agent picker instead of inserting a new header control', async () => {
  let finish!: (value: unknown) => void;
  native(command => command === 'agents_list' ? new Promise(resolve => { finish = resolve; }) : undefined);
  render(<Example />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  const picker = screen.getByRole('combobox', { name: 'Agent' }) as HTMLSelectElement;
  expect(picker.disabled).toBe(true);
  await act(async () => finish([{ id, name: 'reader', path: null, available: true }]));
  await waitFor(() => expect(picker.disabled).toBe(false));
  expect(screen.getByRole('combobox', { name: 'Agent' })).toBe(picker);
});
