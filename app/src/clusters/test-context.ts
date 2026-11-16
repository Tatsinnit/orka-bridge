import { screen, waitFor } from '@testing-library/react';
import type userEvent from '@testing-library/user-event';
import { afterEach, beforeEach } from 'vitest';

beforeEach(() => {
  Object.defineProperty(HTMLDivElement.prototype, 'showPopover', { configurable: true, value() { this.style.display = 'grid'; } });
});
afterEach(() => { Reflect.deleteProperty(HTMLDivElement.prototype, 'showPopover'); });
export async function contextChoice(user: ReturnType<typeof userEvent.setup>, name: string) {
  const trigger = screen.getByRole('button', { name: 'kubectl context' });
  await waitFor(() => { if (trigger.matches(':disabled')) throw Error('Contexts still loading'); });
  if (!screen.queryByRole('dialog', { name: 'Choose a context' })) await user.click(trigger);
  return screen.findByRole('button', { name });
}
export async function chooseContext(user: ReturnType<typeof userEvent.setup>, name: string) {
  await user.click(await contextChoice(user, name));
}

export async function reloadConfig(user: ReturnType<typeof userEvent.setup>) {
  if (!screen.queryByRole('button', { name: 'Reload kubeconfig' })) await user.click(screen.getByRole('button', { name: 'Details' }));
  await user.click(screen.getByRole('button', { name: 'Reload kubeconfig' }));
}
