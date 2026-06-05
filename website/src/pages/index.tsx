import clsx from 'clsx';
import Heading from '@theme/Heading';
import Layout from '@theme/Layout';
import Link from '@docusaurus/Link';

import styles from './index.module.css';

type DocLink = {
  title: string;
  description: string;
  to: string;
};

const docLinks: DocLink[] = [
  {
    title: 'Measurements',
    description: 'Validated commands, benchmark results, workflow smokes, and release gates.',
    to: '/docs/MEASUREMENTS',
  },
  {
    title: 'SDK Positioning',
    description: 'How the agent SDK, native bindings, CLI, and MCP surfaces fit together.',
    to: '/docs/SDK_POSITIONING',
  },
  {
    title: 'SkillOpt Lite',
    description: 'The bounded profile-improvement loop and validation policy for memory skills.',
    to: '/docs/SKILLOPT_LITE',
  },
];

function HomepageHeader() {
  return (
    <header className={styles.hero}>
      <div className="container">
        <p className={styles.eyebrow}>AideMemo Docs</p>
        <Heading as="h1" className={styles.title}>
          Agent-friendly SDK memory for coding agents.
        </Heading>
        <p className={styles.subtitle}>
          Static documentation for the AideMemo CLI, MCP surface, SDK path, and
          measurement-backed workflow gates.
        </p>
        <div className={styles.actions}>
          <Link className="button button--primary" to="/docs/MEASUREMENTS">
            Open Docs
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
