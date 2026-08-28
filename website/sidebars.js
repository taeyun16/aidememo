/** @type {import('@docusaurus/plugin-content-docs').SidebarsConfig} */
const sidebars = {
  docs: [
    {
      type: 'category',
      label: 'Get Started',
      collapsed: false,
      items: ['INTRODUCTION', 'INSTALLATION', 'QUICKSTART', 'HANDOFF', 'ARCHITECTURE'],
    },
    {
      type: 'category',
      label: 'Use AideMemo',
      collapsed: false,
      items: [
        'CLI',
        'MCP',
        'SHARED_MEMORY',
        'SERVER_SSOT',
        'ARTIFACT_CONFORMANCE',
        'CODING_AGENTS',
        'CODEX_MULTI_PROFILE',
        'AGENT_WORKFLOWS',
        'SDK',
        'FEATURES',
        'OPERATIONS',
        'POSTGRES_BACKUP',
        'LFM_EXPERIMENTS',
        'BRANCHES',
        'EVIDENCE',
        'MEASUREMENTS',
        'CROSS_AGENT_DEMO',
        'RELEASE',
      ],
    },
  ],
};

module.exports = sidebars;
