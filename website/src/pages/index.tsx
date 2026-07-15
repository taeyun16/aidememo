import {useState} from 'react';

import Translate, {translate} from '@docusaurus/Translate';
import Link from '@docusaurus/Link';
import useBaseUrl from '@docusaurus/useBaseUrl';
import Heading from '@theme/Heading';
import Layout from '@theme/Layout';

import styles from './index.module.css';

const INSTALL_COMMAND = 'curl -fsSL https://raw.githubusercontent.com/taeyun16/aidememo/main/scripts/install.sh | bash';

const agents = ['HERMES', 'CODEX', 'CLAUDE CODE'];

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
      id: 'homepage.card.codingAgents.title',
      message: 'Coding agent setup',
      description: 'Homepage link title for coding agent setup.',
    }),
    description: translate({
      id: 'homepage.card.codingAgents.description',
      message: 'Install AideMemo for Claude Code, Codex, Hermes, pi, and other agents.',
      description: 'Homepage link description for coding agent setup.',
    }),
    to: '/docs/CODING_AGENTS',
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
      id: 'homepage.card.sharedMemory.title',
      message: 'Shared memory',
      description: 'Homepage link title for the shared-memory deployment guide.',
    }),
    description: translate({
      id: 'homepage.card.sharedMemory.description',
      message: 'Give multiple agents one durable store with explicit source and writer identities.',
      description: 'Homepage link description for the shared-memory deployment guide.',
    }),
    to: '/docs/SHARED_MEMORY',
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
        message: 'Hermes, Codex, and Claude Code continue from one shared project memory.',
      })}
    >
      <div className={styles.relayAgents}>
        {agents.map((agent, index) => (
          <div className={styles.relayAgent} key={agent}>
            <span>{String(index + 1).padStart(2, '0')}</span>
            <strong>{agent}</strong>
          </div>
        ))}
      </div>
      <div className={styles.relayLine} aria-hidden="true">
        <span />
      </div>
      <div className={styles.memoryCore}>
        <img src={logoSrc} alt="" />
        <div>
          <span>SHARED PROJECT MEMORY</span>
          <strong>What failed. Why. What comes next.</strong>
        </div>
      </div>
      <div className={styles.recoveredContext}>
        <span>RECOVERED BY THE NEXT AGENT</span>
        <p><b>ERROR</b> Old refresh token was reused.</p>
        <p><b>LESSON</b> Tokens rotate after every refresh.</p>
        <p><b>DECISION</b> Persist the new token atomically.</p>
      </div>
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
              <Translate id="homepage.eyebrow">Cross-agent project continuity</Translate>
            </span>
          </div>
          <Heading as="h1" className={styles.productName}>
            AideMemo
          </Heading>
          <p className={styles.heroTitle}>
            <Translate id="homepage.hero.title">
              Switch coding agents. Keep the work moving.
            </Translate>
          </p>
          <p className={styles.heroSubtitle}>
            <Translate id="homepage.hero.subtitle">
              Carry decisions, failed attempts, and lessons across Hermes, Codex, Claude Code, and
              other coding agents. No chat sync. No restart from zero. The default local memory
              loop does not require an external LLM call.
            </Translate>
          </p>
          <div className={styles.heroActions}>
            <Link className={styles.primaryAction} to="/docs/INSTALLATION">
              <Translate id="homepage.action.start">Give my agents memory</Translate>
              <span aria-hidden="true">↓</span>
            </Link>
            <Link className={styles.secondaryAction} to="/docs/INTRODUCTION">
              <Translate id="homepage.action.docs">See how continuity works</Translate>
              <Arrow />
            </Link>
          </div>
        </div>
        <MemoryGraph logoSrc={logoSrc} />
      </div>

      <div className={styles.installRail} id="install">
        <span className={styles.installLabel}>
          <Translate id="homepage.install.label">Install AideMemo</Translate>
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
            <Translate id="homepage.profiles.kicker">One project. Any agent.</Translate>
          </p>
          <Heading as="h2" id="profile-continuity-title" className={styles.sectionTitle}>
            <Translate id="homepage.profiles.title">The conversation ends. The work does not.</Translate>
          </Heading>
          <p className={styles.sectionBody}>
            <Translate id="homepage.profiles.body">
              AideMemo does not move chats between tools. It gives the next agent the durable
              project knowledge it needs to continue: what was tried, what failed, and what the
              team decided next.
            </Translate>
          </p>
          <Link className={styles.textLink} to="/docs/CODEX_MULTI_PROFILE">
            <Translate id="homepage.profiles.action">Set up your coding agents</Translate>
            <Arrow />
          </Link>
        </div>
        <div className={styles.profileFlow} aria-label="A failed attempt becomes useful context for the next coding agent">
          <div className={styles.profileNode}>
            <span>YESTERDAY · HERMES</span>
            <strong>Found the failure</strong>
            <small>Old refresh tokens trigger replay detection.</small>
          </div>
          <div className={styles.profileConnector} aria-hidden="true">↘</div>
          <div className={styles.sharedMemoryNode}>
            <span>AIDEMEMO · LOCAL</span>
            <strong>Kept the lesson</strong>
            <small>Error, root cause, and agreed fix stay with the project.</small>
          </div>
          <div className={styles.profileConnector} aria-hidden="true">↘</div>
          <div className={styles.profileNode}>
            <span>TODAY · CLAUDE CODE</span>
            <strong>Continued the fix</strong>
            <small>Started from the atomic token update—not the failed attempt.</small>
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
          <Translate id="homepage.final.title">Your project memory belongs to the project. Not the agent.</Translate>
        </Heading>
        <div className={styles.finalActions}>
          <Link className={styles.primaryAction} to="/docs/QUICKSTART">
            <Translate id="homepage.final.quickstart">Give my agents memory</Translate>
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
        message: 'Cross-agent project continuity',
        description: 'Browser title for the AideMemo product homepage.',
      })}
      description={translate({
        id: 'homepage.meta.description',
        message:
          'Switch between Hermes, Codex, Claude Code, and other coding agents without restarting from zero. AideMemo keeps project decisions, failures, and lessons local.',
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
