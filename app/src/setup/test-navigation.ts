import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// Existing lifecycle tests exercise local controls, now reached through the Places row.
export async function openLocalSetup(user = userEvent.setup()) {
  await waitFor(() => {
    if (!screen.queryByRole('region', { name: 'Workspace' }) && !screen.queryByRole('list', { name: 'Setup steps' }) && !screen.queryByRole('heading', { name: 'Where should your agents run?' }) && !screen.queryByRole('button', { name: 'Back to clusters' }) && !screen.queryByRole('region', { name: 'This computer' })) throw Error('Waiting for Places or local progress');
  });
  const row = screen.queryByRole('region', { name: 'This computer' });
  if (row) await user.click(within(row).getByRole('button', { name: /^(Set up|Continue setup|Connect the AI|Manage|View details|View workspace)$/ }));
  // Lifecycle tests opt into the early AI choices; the dedicated layout test checks the collapsed default.
  const inline = document.querySelector<HTMLElement>('.model-choice-slot .model-inline-head');
  if (inline) {
    const change = within(inline).getByRole('button', { name: 'Change' });
    if (change.getAttribute('aria-expanded') === 'false') await user.click(change);
  }
}
