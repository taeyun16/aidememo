/** @type {import('@docusaurus/plugin-content-docs').SidebarsConfig} */
const sidebars = {
  docs: [
    {
      type: 'category',
      label: 'Get Started',
      collapsed: false,
      items: ['INTRODUCTION', 'ARCHITECTURE', 'INSTALLATION', 'QUICKSTART'],
    },
    {
      type: 'category',
      label: 'Use AideMemo',
      collapsed: false,
      items: [
        'CLI',
        'MCP',
        'SHARED_MEMORY',
        'CODING_AGENTS',
        'CROSS_AGENT_DEMO',
        'CODEX_MULTI_PROFILE',
        'AGENT_WORKFLOWS',
        'SDK',
        'FEATURES',
        'OPERATIONS',
        'LFM_EXPERIMENTS',
        'BRANCHES',
        'EVIDENCE',
        'MEASUREMENTS',
        'RELEASE',
      ],
    },
  ],
};

module.exports = sidebars;
