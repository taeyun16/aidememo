const lightCodeTheme = require('prism-react-renderer').themes.github;
const darkCodeTheme = require('prism-react-renderer').themes.dracula;

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'AideMemo',
  tagline: 'Agent-friendly SDK memory for coding agents.',
  url: 'https://taeyun16.github.io',
  baseUrl: '/aidememo/',
  organizationName: 'taeyun16',
  projectName: 'aidememo',
  trailingSlash: false,
  onBrokenLinks: 'warn',
  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'warn',
    },
  },

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      {
        docs: {
          path: '../docs',
          routeBasePath: 'docs',
          sidebarPath: require.resolve('./sidebars.js'),
          editUrl: ({docPath}) =>
            `https://github.com/taeyun16/aidememo/blob/main/docs/${docPath}`,
        },
        blog: false,
        theme: {
          customCss: require.resolve('./src/css/custom.css'),
        },
      },
    ],
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      metadata: [
        {
          name: 'description',
          content:
            'AideMemo documentation for local-first agent memory, SDKs, MCP tools, and measurement-backed workflows.',
        },
      ],
      navbar: {
        title: 'AideMemo',
        items: [
          {
            type: 'docSidebar',
            sidebarId: 'docs',
            position: 'left',
            label: 'Docs',
          },
          {
            href: 'https://github.com/taeyun16/aidememo',
            label: 'GitHub',
            position: 'right',
          },
        ],
      },
      footer: {
        style: 'dark',
        links: [
          {
            title: 'Docs',
            items: [
              {
                label: 'Measurements',
                to: '/docs/MEASUREMENTS',
              },
              {
                label: 'SDK Positioning',
                to: '/docs/SDK_POSITIONING',
              },
              {
                label: 'SkillOpt Lite',
                to: '/docs/SKILLOPT_LITE',
              },
            ],
          },
          {
            title: 'Project',
            items: [
              {
                label: 'GitHub',
                href: 'https://github.com/taeyun16/aidememo',
              },
              {
                label: 'Compare',
                href: 'https://github.com/taeyun16/aidememo/blob/main/COMPARE.md',
              },
            ],
          },
        ],
        copyright: `Copyright © ${new Date().getFullYear()} AideMemo contributors.`,
      },
      prism: {
        theme: lightCodeTheme,
        darkTheme: darkCodeTheme,
      },
    }),
};

module.exports = config;
