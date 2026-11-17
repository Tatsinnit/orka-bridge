import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

// jsdom does not lay out controls; native fixture screenshots exercise these values.
const css = readFileSync('src/components/controls.css', 'utf8');
describe('shared control styles', () => {
  it('pins the two control sizes and preserves keyboard focus', () => {
    expect(css).toMatch(/\.button\s*\{[^}]*min-height: 32px/s);
    expect(css).toMatch(/\.button-small\s*\{[^}]*min-height: 26px/s);
    expect(css).toMatch(/\.button-icon\s*\{[^}]*width: 26px/s);
    expect(readFileSync('src/app.css', 'utf8')).toContain(':focus-visible { outline: 2px solid var(--color-action); outline-offset: 3px; }');
    expect(css).toMatch(/h1\[tabindex="-1"\]:focus\s*\{\s*outline: none;/);
  });
  it('shares theme-aware field chrome and draws a native select chevron', () => {
    expect(css).toMatch(/\.field\s*\{[^}]*background: var\(--color-card\)/s);
    expect(css).toMatch(/select\.field\s*\{[^}]*appearance: none/s);
    expect(css).toMatch(/input\.field, select\.field\s*\{\s*height: 32px;/);
    expect(readFileSync('src/app.css', 'utf8')).toMatch(/\.tool-picker:popover-open\s*\{[^}]*height: fit-content;[^}]*align-content: start;/s);
    expect(css).toContain('background-image: linear-gradient(');
    expect(css).not.toContain('data:');
    expect(css).toContain('var(--color-text-secondary)');
    expect(css).not.toContain('.agent-editor');
    expect(readFileSync('src/app.css', 'utf8')).toMatch(/\.field\.model-select\s*\{\s*min-width: 12rem/);
  });
  it('removes the six competing action style families', () => {
    for (const path of ['app.css', 'shell.css', 'setup/welcome.css', 'setup/steps.css', 'components/design-system.css']) {
      expect(readFileSync(`src/${path}`, 'utf8')).not.toMatch(/\.(?:shell-button|setup-button|part-button|workspace-primary|workspace-secondary|workspace-link)\b/);
    }
  });
});
