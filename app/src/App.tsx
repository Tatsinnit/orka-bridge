import { useState } from 'react';
import mark from './assets/orka-mark.png';

const pages = ['Overview', 'Designer', 'Clusters'] as const;
type Page = (typeof pages)[number];

const emptyCopy: Record<Page, { title: string; detail: string }> = {
  Overview: {
    title: 'Your agents, wherever they run',
    detail: 'Agents on this machine and your connected clusters will appear here.',
  },
  Designer: {
    title: 'A place to shape your agent',
    detail: 'Your agent file, prompt, tools and tests will live here.',
  },
  Clusters: {
    title: 'A place for your agents to run',
    detail: 'Your local Orka and connected clusters will appear here.',
  },
};

function PageIcon({ page }: { page: Page }) {
  return (
    <svg className="icon" viewBox="0 0 24 24" aria-hidden="true">
      {page === 'Overview' && <>
        <rect x="3" y="4" width="18" height="4.5" rx="1.5" />
        <rect x="3" y="10" width="18" height="4.5" rx="1.5" />
        <rect x="3" y="16" width="18" height="4.5" rx="1.5" />
      </>}
      {page === 'Designer' && <path d="M4 20l1-4L16 5a2.1 2.1 0 0 1 3 3L8 19l-4 1zM14 7l3 3" />}
      {page === 'Clusters' && <path d="M12 3 3 8l9 5 9-5-9-5zM3 13l9 5 9-5" />}
    </svg>
  );
}

export default function App() {
  const [page, setPage] = useState<Page>('Overview');
  const copy = emptyCopy[page];
  return (
    <div className="app-shell">
      <header className="tab-bar">
        <span className="brand"><img src={mark} width="32" height="36" alt="Orka" /><b>Bridge</b></span>
        <nav className="page-nav" aria-label="Main">
          {pages.map(name => (
            <button key={name} className="page-tab" type="button"
              aria-current={page === name ? 'page' : undefined}
              onClick={() => setPage(name)}>
              <span className="icon-tile"><PageIcon page={name} /></span>
              <span>{name}</span>
            </button>
          ))}
        </nav>
      </header>
      <main className="page-content" aria-labelledby="page-title">
        <h1 id="page-title">{page}</h1>
        <section className="card empty-state">
          <span className="icon-tile empty-icon"><PageIcon page={page} /></span>
          <div>
            <h2>{copy.title}</h2>
            <p className="empty-status">Nothing is connected yet.</p>
            <p className="muted">{copy.detail}</p>
          </div>
        </section>
      </main>
    </div>
  );
}
