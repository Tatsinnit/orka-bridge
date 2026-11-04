import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const css = readFileSync('src/app.css', 'utf8');
const palettes = [...css.matchAll(/:root\s*\{([^}]+)\}/g)].map(([, block]) =>
  Object.fromEntries([...block.matchAll(/--([\w-]+):\s*([^;]+);/g)].map(([, name, value]) => [name, value.trim()])),
);

describe('Tuxedo theme contract', () => {
  it.each([
    ['light', 0, '#FFFFFF', '#0B0B0C', '#0369A1', '#15803D'],
    ['dark', 1, '#0E0E10', '#F4F4F5', '#4CB8F5', '#4ADE80'],
  ] as const)('%s keeps neutral chrome and Orka phase meanings', (_, index, page, ink, running, succeeded) => {
    expect(palettes[index]).toMatchObject({
      'color-page': page, 'color-text': ink, 'color-action': ink,
      'color-status-running': running, 'color-status-succeeded': succeeded,
    });
  });
  it('uses the system dark preference and bundled Geist families', () => {
    expect(css).toContain('@media (prefers-color-scheme: dark)');
    expect(palettes[0]['font-ui']).toContain('Geist Variable');
    expect(palettes[0]['font-mono']).toContain('Geist Mono Variable');
    expect(css).not.toContain('Estuary');
  });
});
