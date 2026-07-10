import clsx from 'clsx';
import Translate, {translate} from '@docusaurus/Translate';
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
    title: translate({
      id: 'homepage.card.start.title',
      message: 'Start Here',
      description: 'Homepage card title linking to the AideMemo introduction.',
    }),
    description: translate({
      id: 'homepage.card.start.description',
      message: 'Learn what AideMemo is, when to use it, and how it fits your agent workflow.',
      description: 'Homepage card description for the AideMemo introduction.',
    }),
    to: '/docs/INTRODUCTION',
  },
  {
    title: translate({
      id: 'homepage.card.quickstart.title',
      message: 'Quickstart',
      description: 'Homepage card title linking to the quickstart.',
    }),
    description: translate({
      id: 'homepage.card.quickstart.description',
      message: 'Add facts, search memory, and start a workflow from a sparse ticket.',
      description: 'Homepage card description for the quickstart.',
    }),
    to: '/docs/QUICKSTART',
  },
  {
    title: translate({
      id: 'homepage.card.architecture.title',
      message: 'Architecture',
      description: 'Homepage card title linking to the architecture guide.',
    }),
    description: translate({
      id: 'homepage.card.architecture.description',
      message: 'See how CLI, MCP, the agent SDK, bindings, core, stores, and indexes fit together.',
      description: 'Homepage card description for the architecture guide.',
    }),
    to: '/docs/ARCHITECTURE',
  },
  {
    title: translate({
      id: 'homepage.card.mcp.title',
      message: 'MCP Setup',
      description: 'Homepage card title linking to MCP setup.',
    }),
    description: translate({
      id: 'homepage.card.mcp.description',
      message: 'Register AideMemo with local agents and use the core memory tools.',
      description: 'Homepage card description for MCP setup.',
    }),
    to: '/docs/MCP',
  },
  {
    title: translate({
      id: 'homepage.card.workflows.title',
      message: 'Agent Workflows',
      description: 'Homepage card title linking to agent workflows.',
    }),
    description: translate({
      id: 'homepage.card.workflows.description',
      message: 'Choose workflow, context, query, aggregate, canvas, and profile calls by task shape.',
      description: 'Homepage card description for agent workflows.',
    }),
    to: '/docs/AGENT_WORKFLOWS',
  },
  {
    title: translate({
      id: 'homepage.card.evidence.title',
      message: 'Evidence',
      description: 'Homepage card title linking to the evidence scorecard.',
    }),
    description: translate({
      id: 'homepage.card.evidence.description',
      message: 'Review validated outcomes, model placement, and the boundaries of public claims.',
      description: 'Homepage card description for the evidence scorecard.',
    }),
    to: '/docs/EVIDENCE',
  },
];

function HomepageHeader() {
  const logoSrc = useBaseUrl('img/aidememo-logo.png');

  return (
    <header className={styles.hero}>
      <div className="container">
        <img
          className={styles.logoMark}
          src={logoSrc}
          alt={translate({
            id: 'homepage.logo.alt',
            message: 'AideMemo logo',
            description: 'Alternative text for the AideMemo logo on the homepage.',
          })}
        />
        <p className={styles.eyebrow}>
          <Translate id="homepage.eyebrow">AideMemo Docs</Translate>
        </p>
        <Heading as="h1" className={styles.title}>
          <Translate id="homepage.hero.title">
            Agent-friendly SDK memory for coding agents.
          </Translate>
        </Heading>
        <p className={styles.subtitle}>
          <Translate id="homepage.hero.subtitle">
            Learn how to install AideMemo, add useful memory, connect it to agents through MCP,
            and use the agent SDK from scripts. The default local memory loop does not require an
            external LLM call.
          </Translate>
        </p>
        <div className={styles.actions}>
          <Link className="button button--primary" to="/docs/INTRODUCTION">
            <Translate id="homepage.action.start">Start Reading</Translate>
          </Link>
          <Link className="button button--secondary" to="https://github.com/taeyun16/aidememo">
            <Translate id="homepage.action.github">View GitHub</Translate>
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
      title={translate({
        id: 'homepage.meta.title',
        message: 'AideMemo Documentation',
        description: 'Browser title for the AideMemo documentation homepage.',
      })}
      description={translate({
        id: 'homepage.meta.description',
        message: 'Static documentation for AideMemo agent memory.',
        description: 'Meta description for the AideMemo documentation homepage.',
      })}
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
