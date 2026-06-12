/** @type {import('@docusaurus/plugin-content-docs').SidebarsConfig} */
const sidebars = {
  docs: [
    {
      type: 'category',
      label: 'Get Started',
      collapsed: false,
      items: ['INTRODUCTION', 'INSTALLATION', 'QUICKSTART'],
    },
    {
      type: 'category',
      label: 'Use AideMemo',
      collapsed: false,
      items: ['CLI', 'MCP', 'SDK', 'FEATURES', 'OPERATIONS', 'RELEASE'],
    },
  ],
};

module.exports = sidebars;
