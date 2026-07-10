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
        'AGENT_WORKFLOWS',
        'SDK',
        'FEATURES',
        'OPERATIONS',
        'BRANCHES',
        'EVIDENCE',
        'MEASUREMENTS',
        'RELEASE',
      ],
    },
  ],
};

module.exports = sidebars;
