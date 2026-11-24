import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const css = readFileSync('src/app.css', 'utf8');
const palettes = [...css.matchAll(/:root\s*\{([^}]+)\}/g)].map(([, block]) =>
  Object.fromEntries([...block.matchAll(/--([\w-]+):\s*([^;]+);/g)].map(([, name, value]) => [name, value.trim()])),
);

const luminance = (hex: string) => {
  const rgb = hex.slice(1).match(/../g)!.map(channel => parseInt(channel, 16) / 255)
    .map(channel => channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4);
  return rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
};
const contrast = (foreground: string, background: string) => {
  const [lighter, darker] = [luminance(foreground), luminance(background)].sort((a, b) => b - a);
  return (lighter + 0.05) / (darker + 0.05);
};

describe('Orka ink and paper theme contract', () => {
  it.each([
    ['light', 0, '#ffffff', '#f6f6f7', '#f0f0f2', '#0b0b0c', '#a64b00', '#0369a1', '#047857', '#be123c'],
    ['dark', 1, '#09090a', '#1b1b1e', '#1b1b1e', '#f4f4f5', '#f5b544', '#4cb8f5', '#4ade80', '#f87171'],
  ] as const)('%s uses the Orka neutrals and state colours', (_, index, page, surface, soft, ink, pending, running, succeeded, failed) => {
    expect(palettes[index]).toMatchObject({
      'color-page': page, 'color-chrome': surface, 'color-hover': soft,
      'color-text': ink, 'color-action': ink, 'color-status-pending': pending,
      'color-status-running': running, 'color-status-succeeded': succeeded, 'color-status-failed': failed,
    });
  });
  it.each([0, 1])('keeps every text role readable on shared surfaces in palette %s', index => {
    const palette = palettes[index];
    for (const ink of ['color-text-secondary', 'color-text-tertiary', 'color-status-pending', 'color-status-running', 'color-status-succeeded', 'color-status-failed']) {
      for (const surface of ['color-page', 'color-card', 'color-chrome', 'color-hover']) {
        expect(contrast(palette[ink], palette[surface])).toBeGreaterThanOrEqual(4.5);
      }
    }
    expect(contrast(palette['color-action-text'], palette['color-action'])).toBeGreaterThanOrEqual(4.5);
    expect(contrast(palette['color-action-text'], palette['color-action-hover'])).toBeGreaterThanOrEqual(4.5);
    expect(contrast(palette['color-terminal-text'], palette['color-terminal'])).toBeGreaterThanOrEqual(4.5);
  });
  it.each([0, 1])('keeps every status tint readable in palette %s', index => {
    const palette = palettes[index];
    for (const [ink, surface] of [
      ['color-status-pending', 'color-tint-pending'],
      ['color-status-running', 'color-tint-running'],
      ['color-status-succeeded', 'color-tint-succeeded'],
      ['color-status-failed', 'color-tint-failed'],
      ['color-text-secondary', 'color-tint-stale'],
    ]) expect(contrast(palette[ink], palette[surface])).toBeGreaterThanOrEqual(4.5);
  });
  it('declares themed thin scrollbars and non-growing wide tabs', () => {
    const shell = readFileSync('src/shell.css', 'utf8');
    const controls = readFileSync('src/components/controls.css', 'utf8');
    expect(shell).toMatch(/scrollbar-width:\s*thin/);
    expect(shell).toMatch(/scrollbar-color:\s*var\(--color-text-tertiary\)\s+var\(--color-page\)/);
    expect(shell).toMatch(/\.page-tab\s*\{[^}]*flex:\s*0 1 180px/);
    expect(controls).toMatch(/\.button-quiet\s*\{[^}]*padding-inline:\s*0/);
  });
  it('uses the system dark preference and bundled reading and code families', () => {
    expect(css).toContain('@media (prefers-color-scheme: dark)');
    expect(palettes[0]['font-ui']).toContain('Geist Variable');
    expect(palettes[0]['font-mono']).toContain('Geist Mono Variable');
  });
  it('uses one six-pixel radius and reserves colour for state', () => {
    expect(palettes[0]).toMatchObject({
      radius: '6px',
      'radius-control': 'var(--radius)',
      'radius-row': 'var(--radius)',
      'radius-card': 'var(--radius)',
    });
    const designSystem = readFileSync('src/components/design-system.css', 'utf8');
    const shell = readFileSync('src/shell.css', 'utf8');
    const controls = readFileSync('src/components/controls.css', 'utf8');
    expect(designSystem).not.toContain('border-radius: 999px');
    expect(shell).toMatch(/\.tile-overview, \.tile-designer, \.tile-clusters\s*\{[^}]*color: var\(--color-text\);[^}]*background: var\(--color-icon-tile\)/s);
    expect(controls).toMatch(/\.button-danger\s*\{\s*color: var\(--color-text\);/);
    expect(css).toMatch(/input:is\(\[type='radio'\], \[type='checkbox'\]\)\s*\{\s*accent-color: var\(--color-text\);/);
    expect(css).toMatch(/a\s*\{\s*color: var\(--color-text\);/);
    expect(css).not.toContain('color-violet');
  });
});
