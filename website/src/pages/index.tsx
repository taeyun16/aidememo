import {useState} from 'react';

import Translate, {translate} from '@docusaurus/Translate';
import Link from '@docusaurus/Link';
import useBaseUrl from '@docusaurus/useBaseUrl';
import Heading from '@theme/Heading';
import Layout from '@theme/Layout';

import styles from './index.module.css';

const INSTALL_COMMAND =
  'cargo install --git https://github.com/taeyun16/aidememo aidememo-cli';

type WorkflowStep = {
  number: string;
  label: string;
  title: string;
  description: string;
  command: string;
};

type InterfaceLink = {
  name: string;
  detail: string;
  to: string;
};

type DocLink = {
  title: string;
  description: string;
  to: string;
};

const workflowSteps: WorkflowStep[] = [
  {
    number: '01',
    label: translate({id: 'homepage.workflow.capture.label', message: 'Capture'}),
    title: translate({
      id: 'homepage.workflow.capture.title',
      message: 'Keep the facts that should outlive this session.',
    }),
    description: translate({
      id: 'homepage.workflow.capture.description',
      message:
        'Decisions, lessons, errors, preferences, and relations stay explicit instead of disappearing into a transcript.',
    }),
    command: 'aidememo fact add "Use Redis Cluster" \\\n  --type decision --entities Redis,Cache',
  },
  {
    number: '02',
    label: translate({id: 'homepage.workflow.retrieve.label', message: 'Retrieve'}),
    title: translate({
      id: 'homepage.workflow.retrieve.title',
      message: 'Recall the right context before the next plan.',
    }),
    description: translate({
      id: 'homepage.workflow.retrieve.description',
      message:
        'BM25-first search stays fast and deterministic, while semantic retrieval steps in when lexical evidence is weak.',
    }),
    command: 'aidememo query "Redis cache" --depth 2',
  },
  {
    number: '03',
    label: translate({id: 'homepage.workflow.apply.label', message: 'Apply'}),
    title: translate({
      id: 'homepage.workflow.apply.title',
      message: 'Bring memory into the tool already doing the work.',
    }),
    description: translate({
      id: 'homepage.workflow.apply.description',
      message:
        'Use the same local store through MCP, the agent SDK, the CLI, or native bindings without adopting a hosted runtime.',
    }),
    command: 'aidememo init --agent codex ./wiki',
  },
];

const interfaceLinks: InterfaceLink[] = [
  {
    name: 'MCP',
    detail: translate({
      id: 'homepage.interface.mcp',
      message: 'Model-visible tools over stdio or HTTP',
    }),
    to: '/docs/MCP',
  },
  {
    name: 'Agent SDK',
    detail: translate({
      id: 'homepage.interface.sdk',
      message: 'Code-first memory composition from Python',
    }),
    to: '/docs/SDK',
  },
  {
    name: 'CLI',
    detail: translate({
      id: 'homepage.interface.cli',
      message: 'Compact commands for humans and automation',
    }),
    to: '/docs/CLI',
  },
  {
    name: 'Native',
    detail: translate({
      id: 'homepage.interface.native',
      message: 'Python, Node, Elixir, and C bindings',
    }),
    to: '/docs/ARCHITECTURE',
  },
];

const docLinks: DocLink[] = [
  {
    title: translate({
      id: 'homepage.card.start.title',
      message: 'Start here',
      description: 'Homepage link title for the AideMemo introduction.',
    }),
    description: translate({
      id: 'homepage.card.start.description',
      message: 'Understand the product boundary and the basic memory model.',
      description: 'Homepage link description for the AideMemo introduction.',
    }),
    to: '/docs/INTRODUCTION',
  },
  {
    title: translate({
      id: 'homepage.card.quickstart.title',
      message: 'Quickstart',
      description: 'Homepage link title for the quickstart.',
    }),
    description: translate({
      id: 'homepage.card.quickstart.description',
      message: 'Create a store, add typed facts, and recover them from a ticket.',
      description: 'Homepage link description for the quickstart.',
    }),
    to: '/docs/QUICKSTART',
  },
  {
    title: translate({
      id: 'homepage.card.architecture.title',
      message: 'Architecture',
      description: 'Homepage link title for the architecture guide.',
    }),
    description: translate({
      id: 'homepage.card.architecture.description',
      message: 'See how the interfaces, Rust core, stores, and indexes fit together.',
      description: 'Homepage link description for the architecture guide.',
    }),
    to: '/docs/ARCHITECTURE',
  },
  {
    title: translate({
      id: 'homepage.card.mcp.title',
      message: 'MCP setup',
      description: 'Homepage link title for MCP setup.',
    }),
    description: translate({
      id: 'homepage.card.mcp.description',
      message: 'Register AideMemo with local agents and use the core tools.',
      description: 'Homepage link description for MCP setup.',
    }),
    to: '/docs/MCP',
  },
  {
    title: translate({
      id: 'homepage.card.workflows.title',
      message: 'Agent workflows',
      description: 'Homepage link title for agent workflows.',
    }),
    description: translate({
      id: 'homepage.card.workflows.description',
      message: 'Choose context, query, workflow, and aggregate calls by task shape.',
      description: 'Homepage link description for agent workflows.',
    }),
    to: '/docs/AGENT_WORKFLOWS',
  },
  {
    title: translate({
      id: 'homepage.card.codexProfiles.title',
      message: 'Multiple Codex accounts',
      description: 'Homepage link title for the Codex multi-profile use case.',
    }),
    description: translate({
      id: 'homepage.card.codexProfiles.description',
      message: 'Share one project memory while login state and writer provenance stay separate.',
      description: 'Homepage link description for the Codex multi-profile use case.',
    }),
    to: '/docs/CODEX_MULTI_PROFILE',
  },
  {
    title: translate({
      id: 'homepage.card.evidence.title',
      message: 'Evidence',
      description: 'Homepage link title for the evidence scorecard.',
    }),
    description: translate({
      id: 'homepage.card.evidence.description',
      message: 'Read the measured outcomes, caveats, and claim boundaries.',
      description: 'Homepage link description for the evidence scorecard.',
    }),
    to: '/docs/EVIDENCE',
  },
];

function Arrow(): JSX.Element {
  return <span aria-hidden="true">↗</span>;
}

function CopyCommand(): JSX.Element {
  const [copied, setCopied] = useState(false);

  async function copy(): Promise<void> {
    if (!navigator.clipboard) {
      return;
    }
    await navigator.clipboard.writeText(INSTALL_COMMAND);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1800);
  }

  return (
    <button className={styles.copyButton} type="button" onClick={() => void copy()}>
      {copied ? (
        <Translate id="homepage.install.copied">Copied</Translate>
      ) : (
        <Translate id="homepage.install.copy">Copy</Translate>
      )}
    </button>
  );
}

function MemoryGraph({logoSrc}: {logoSrc: string}): JSX.Element {
  return (
    <div
      className={styles.graphVisual}
      role="img"
      aria-label={translate({
        id: 'homepage.graph.alt',
        message: 'AideMemo connects agents to typed facts and one embedded store.',
      })}
    >
      <svg viewBox="0 0 720 720" aria-hidden="true">
        <circle className={styles.orbitOuter} cx="360" cy="360" r="276" />
        <circle className={styles.orbitInner} cx="360" cy="360" r="176" />
        <path className={styles.signalPath} d="M360 360 L154 182" />
        <path className={styles.signalPath} d="M360 360 L568 174" />
        <path className={styles.signalPath} d="M360 360 L604 426" />
        <path className={styles.signalPath} d="M360 360 L468 612" />
        <path className={styles.signalPath} d="M360 360 L122 502" />

        <g className={styles.graphNode} transform="translate(154 182)">
          <circle className={styles.nodeHalo} r="38" />
          <circle className={styles.nodeCore} r="8" />
          <text x="-4" y="-56" textAnchor="middle">AGENT</text>
        </g>
        <g className={styles.graphNode} transform="translate(568 174)">
          <circle className={styles.nodeHalo} r="30" />
          <circle className={styles.nodeCore} r="7" />
          <text x="4" y="-48" textAnchor="middle">DECISION</text>
        </g>
        <g className={styles.graphNode} transform="translate(604 426)">
          <circle className={styles.nodeHalo} r="30" />
          <circle className={styles.nodeCore} r="7" />
          <text x="6" y="54" textAnchor="middle">LESSON</text>
        </g>
        <g className={styles.graphNode} transform="translate(468 612)">
          <circle className={styles.nodeHalo} r="30" />
          <circle className={styles.nodeCore} r="7" />
          <text x="0" y="54" textAnchor="middle">ERROR</text>
        </g>
        <g className={styles.graphNode} transform="translate(122 502)">
          <circle className={styles.nodeHalo} r="38" />
          <circle className={styles.nodeCore} r="8" />
          <text x="0" y="58" textAnchor="middle">SQLITE</text>
        </g>

        <circle className={styles.centerHalo} cx="360" cy="360" r="112" />
        <circle className={styles.centerCore} cx="360" cy="360" r="76" />
        <image href={logoSrc} x="301" y="301" width="118" height="118" />
      </svg>
      <p className={styles.graphCaption}>
        <span>FACTS</span>
        <span>RELATIONS</span>
        <span>HISTORY</span>
      </p>
    </div>
  );
}

function HomepageHero(): JSX.Element {
  const logoSrc = useBaseUrl('img/aidememo-logo.png');

  return (
    <header className={styles.hero}>
      <div className={styles.heroGlow} aria-hidden="true" />
      <div className={styles.heroGrid}>
        <div className={styles.heroCopy}>
          <div className={styles.brandLine}>
            <img
              className={styles.logoMark}
              src={logoSrc}
              alt={translate({
                id: 'homepage.logo.alt',
                message: 'AideMemo logo',
                description: 'Alternative text for the AideMemo logo on the homepage.',
              })}
            />
            <span>
              <Translate id="homepage.eyebrow">Local memory for coding agents</Translate>
            </span>
          </div>
          <Heading as="h1" className={styles.productName}>
            AideMemo
          </Heading>
          <p className={styles.heroTitle}>
            <Translate id="homepage.hero.title">
              Agent-friendly SDK memory for coding agents.
            </Translate>
          </p>
          <p className={styles.heroSubtitle}>
            <Translate id="homepage.hero.subtitle">
              Project memory that survives sessions, editors, and model providers. One Rust binary,
              one embedded store, and a default local loop that does not require an external LLM
              call.
            </Translate>
          </p>
          <div className={styles.heroActions}>
            <Link className={styles.primaryAction} to="/docs/INSTALLATION">
              <Translate id="homepage.action.start">Install from Git</Translate>
              <span aria-hidden="true">↓</span>
            </Link>
            <Link className={styles.secondaryAction} to="/docs/INTRODUCTION">
              <Translate id="homepage.action.docs">Read the docs</Translate>
              <Arrow />
            </Link>
          </div>
        </div>
        <MemoryGraph logoSrc={logoSrc} />
      </div>

      <div className={styles.installRail} id="install">
        <span className={styles.installLabel}>
          <Translate id="homepage.install.label">Install from source</Translate>
        </span>
        <code>{INSTALL_COMMAND}</code>
        <CopyCommand />
      </div>
    </header>
  );
}

function ProofSection(): JSX.Element {
  return (
    <section className={styles.proofSection} aria-labelledby="proof-title">
      <div className={styles.sectionShell}>
        <div className={styles.proofIntro}>
          <p className={styles.sectionKicker}>
            <Translate id="homepage.proof.kicker">Measured, with boundaries</Translate>
          </p>
          <Heading as="h2" id="proof-title" className={styles.sectionTitle}>
            <Translate id="homepage.proof.title">Evidence before adjectives.</Translate>
          </Heading>
          <p className={styles.sectionBody}>
            <Translate id="homepage.proof.body">
              Public claims stay attached to a dataset, retrieval stack, and execution envelope.
            </Translate>
          </p>
          <Link className={styles.textLink} to="/docs/EVIDENCE">
            <Translate id="homepage.proof.action">Read the scorecard</Translate>
            <Arrow />
          </Link>
        </div>
        <div className={styles.metrics}>
          <div className={styles.metric}>
            <strong>0.992</strong>
            <span>R@10</span>
            <p>
              <Translate id="homepage.metric.recall">
                LongMemEval-S, 500 questions, opt-in BGE plus two-stage rerank
              </Translate>
            </p>
          </div>
          <div className={styles.metric}>
            <strong>5.7×</strong>
            <span>
              <Translate id="homepage.metric.faster">faster</Translate>
            </span>
            <p>
              <Translate id="homepage.metric.daemon">
                Daemon BM25 versus fresh CLI, with the same BrainBench score
              </Translate>
            </p>
          </div>
          <div className={styles.metric}>
            <strong>18.4</strong>
            <span>ms p50</span>
            <p>
              <Translate id="homepage.metric.mcp">
                Shared HTTP MCP, two clients and twenty persisted writes
              </Translate>
            </p>
          </div>
        </div>
      </div>
    </section>
  );
}

function WorkflowSection(): JSX.Element {
  return (
    <section className={styles.workflowSection} aria-labelledby="workflow-title">
      <div className={styles.sectionShell}>
        <div className={styles.workflowIntro}>
          <p className={styles.sectionKicker}>
            <Translate id="homepage.workflow.kicker">The local memory loop</Translate>
          </p>
          <Heading as="h2" id="workflow-title" className={styles.sectionTitle}>
            <Translate id="homepage.workflow.title">Memory that stays close to the work.</Translate>
          </Heading>
          <p className={styles.sectionBody}>
            <Translate id="homepage.workflow.body">
              Capture explicitly, retrieve deliberately, and expose only the context the next task
              needs.
            </Translate>
          </p>
        </div>
        <div className={styles.workflowSteps}>
          {workflowSteps.map((step) => (
            <article className={styles.workflowStep} key={step.number}>
              <div className={styles.stepMeta}>
                <span>{step.number}</span>
                <span>{step.label}</span>
              </div>
              <Heading as="h3">{step.title}</Heading>
              <p>{step.description}</p>
              <code>{step.command}</code>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}

function ProfileContinuitySection(): JSX.Element {
  return (
    <section className={styles.profileSection} aria-labelledby="profile-continuity-title">
      <div className={styles.sectionShell}>
        <div className={styles.profileCopy}>
          <p className={styles.sectionKicker}>
            <Translate id="homepage.profiles.kicker">Featured use case</Translate>
          </p>
          <Heading as="h2" id="profile-continuity-title" className={styles.sectionTitle}>
            <Translate id="homepage.profiles.title">Switch accounts, not context.</Translate>
          </Heading>
          <p className={styles.sectionBody}>
            <Translate id="homepage.profiles.body">
              Let isolated Codex profiles share project decisions, lessons, and errors without
              sharing credentials, cookies, or chat history.
            </Translate>
          </p>
          <Link className={styles.textLink} to="/docs/CODEX_MULTI_PROFILE">
            <Translate id="homepage.profiles.action">See the multi-profile setup</Translate>
            <Arrow />
          </Link>
        </div>
        <div className={styles.profileFlow} aria-label="Two Codex profiles share one AideMemo store">
          <div className={styles.profileNode}>
            <span>CODEX</span>
            <strong>Account A</strong>
            <small>actor: codex:account-a</small>
          </div>
          <div className={styles.profileConnector} aria-hidden="true">→</div>
          <div className={styles.sharedMemoryNode}>
            <span>AIDEMEMO</span>
            <strong>Project memory</strong>
            <small>source: project:aidememo</small>
          </div>
          <div className={styles.profileConnector} aria-hidden="true">←</div>
          <div className={styles.profileNode}>
            <span>CODEX</span>
            <strong>Account B</strong>
            <small>actor: codex:account-b</small>
          </div>
        </div>
      </div>
    </section>
  );
}

function InterfacesSection(): JSX.Element {
  return (
    <section className={styles.interfacesSection} aria-labelledby="interfaces-title">
      <div className={styles.sectionShell}>
        <div>
          <p className={styles.sectionKicker}>
            <Translate id="homepage.interfaces.kicker">One core, several entry points</Translate>
          </p>
          <Heading as="h2" id="interfaces-title" className={styles.sectionTitle}>
            <Translate id="homepage.interfaces.title">Meet the agent where it runs.</Translate>
          </Heading>
        </div>
        <div className={styles.interfaceList}>
          {interfaceLinks.map((item) => (
            <Link className={styles.interfaceRow} to={item.to} key={item.name}>
              <span className={styles.interfaceName}>{item.name}</span>
              <span className={styles.interfaceDetail}>{item.detail}</span>
              <Arrow />
            </Link>
          ))}
        </div>
      </div>
    </section>
  );
}

function DocsSection(): JSX.Element {
  return (
    <section className={styles.docsSection} aria-labelledby="docs-title">
      <div className={styles.sectionShell}>
        <div className={styles.docsHeading}>
          <div>
            <p className={styles.sectionKicker}>
              <Translate id="homepage.docs.kicker">Documentation</Translate>
            </p>
            <Heading as="h2" id="docs-title" className={styles.sectionTitle}>
              <Translate id="homepage.docs.title">Go from first fact to full integration.</Translate>
            </Heading>
          </div>
          <p className={styles.sectionBody}>
            <Translate id="homepage.docs.body">
              English and Korean guides share the same tested routes, examples, and release gates.
            </Translate>
          </p>
        </div>
        <div className={styles.docList}>
          {docLinks.map((item, index) => (
            <Link className={styles.docRow} to={item.to} key={item.to}>
              <span className={styles.docNumber}>{String(index + 1).padStart(2, '0')}</span>
              <span className={styles.docCopy}>
                <strong>{item.title}</strong>
                <span>{item.description}</span>
              </span>
              <Arrow />
            </Link>
          ))}
        </div>
      </div>
    </section>
  );
}

function FinalCallToAction(): JSX.Element {
  return (
    <section className={styles.finalCta} aria-labelledby="final-title">
      <div className={styles.finalCtaInner}>
        <p className={styles.sectionKicker}>AIDEMEMO / 0.1</p>
        <Heading as="h2" id="final-title">
          <Translate id="homepage.final.title">Give the next session a better starting point.</Translate>
        </Heading>
        <div className={styles.finalActions}>
          <Link className={styles.primaryAction} to="/docs/QUICKSTART">
            <Translate id="homepage.final.quickstart">Run the quickstart</Translate>
            <Arrow />
          </Link>
          <Link className={styles.secondaryAction} to="https://github.com/taeyun16/aidememo">
            <Translate id="homepage.action.github">View on GitHub</Translate>
            <Arrow />
          </Link>
        </div>
      </div>
    </section>
  );
}

export default function Home(): JSX.Element {
  return (
    <Layout
      title={translate({
        id: 'homepage.meta.title',
        message: 'Local memory for coding agents',
        description: 'Browser title for the AideMemo product homepage.',
      })}
      description={translate({
        id: 'homepage.meta.description',
        message:
          'AideMemo is a local memory layer for coding agents, available through an agent SDK, MCP, CLI, and native bindings.',
        description: 'Meta description for the AideMemo product homepage.',
      })}
    >
      <main className={styles.landing}>
        <HomepageHero />
        <ProofSection />
        <ProfileContinuitySection />
        <WorkflowSection />
        <InterfacesSection />
        <DocsSection />
        <FinalCallToAction />
      </main>
    </Layout>
  );
}
