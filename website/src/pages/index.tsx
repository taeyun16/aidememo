import clsx from 'clsx';
import Heading from '@theme/Heading';
import Layout from '@theme/Layout';
import Link from '@docusaurus/Link';
import useBaseUrl from '@docusaurus/useBaseUrl';

import styles from './index.module.css';

type DocLink = {
  title: string;
  description: string;
  to: string;
};

const docLinks: DocLink[] = [
  {
    title: 'Start Here',
    description: 'Learn what AideMemo is, when to use it, and how it fits your agent workflow.',
    to: '/docs/INTRODUCTION',
  },
  {
    title: 'Quickstart',
    description: 'Add facts, search memory, and start a workflow from a sparse ticket.',
    to: '/docs/QUICKSTART',
  },
  {
    title: 'Architecture',
    description: 'See how CLI, MCP, SDKs, bindings, core, stores, and indexes fit together.',
    to: '/docs/ARCHITECTURE',
  },
  {
    title: 'MCP Setup',
    description: 'Register AideMemo with local agents and use the core memory tools.',
    to: '/docs/MCP',
  },
  {
    title: 'Agent Workflows',
    description: 'Choose workflow, context, query, aggregate, canvas, and profile calls by task shape.',
    to: '/docs/AGENT_WORKFLOWS',
  },
  {
    title: 'Measurements',
    description: 'Review the validation ledger behind workflow, SDK, retrieval, and sharing claims.',
    to: '/docs/MEASUREMENTS',
  },
];

function HomepageHeader() {
  const logoSrc = useBaseUrl('img/aidememo-logo.png');

  return (
    <header className={styles.hero}>
      <div className="container">
        <img className={styles.logoMark} src={logoSrc} alt="AideMemo logo" />
        <p className={styles.eyebrow}>AideMemo Docs</p>
        <Heading as="h1" className={styles.title}>
          Agent-friendly SDK memory for coding agents.
        </Heading>
        <p className={styles.subtitle}>
          Learn how to install AideMemo, add useful memory, connect it to agents
          through MCP, and use the SDK from scripts.
        </p>
        <div className={styles.actions}>
          <Link className="button button--primary" to="/docs/INTRODUCTION">
            Start Reading
          </Link>
          <Link className="button button--secondary" to="https://github.com/taeyun16/aidememo">
            View GitHub
          </Link>
        </div>
      </div>
    </header>
  );
}

function DocCard({title, description, to}: DocLink) {
  return (
    <Link className={clsx(styles.card)} to={to}>
      <span className={styles.cardTitle}>{title}</span>
      <span className={styles.cardDescription}>{description}</span>
    </Link>
  );
}

export default function Home(): JSX.Element {
  return (
    <Layout
      title="AideMemo Documentation"
      description="Static documentation for AideMemo agent memory."
    >
      <HomepageHeader />
      <main className={styles.main}>
        <section className="container">
          <div className={styles.grid}>
            {docLinks.map((props) => (
              <DocCard key={props.title} {...props} />
            ))}
          </div>
        </section>
      </main>
    </Layout>
  );
}
