const lightCodeTheme = require('prism-react-renderer').themes.github;
const darkCodeTheme = require('prism-react-renderer').themes.dracula;

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'AideMemo',
  tagline: 'Local memory for coding agents.',
  url: 'https://taeyun16.github.io',
  baseUrl: '/aidememo/',
  favicon: 'img/aidememo-logo.png',
  organizationName: 'taeyun16',
  projectName: 'aidememo',
  trailingSlash: false,
  onBrokenLinks: 'throw',
  markdown: {
    mermaid: true,
    hooks: {
      onBrokenMarkdownLinks: 'throw',
    },
  },

  i18n: {
    defaultLocale: 'en',
    locales: ['en', 'ko'],
    localeConfigs: {
      en: {
        label: 'English',
        htmlLang: 'en-US',
      },
      ko: {
        label: '한국어',
        htmlLang: 'ko-KR',
      },
    },
  },

  presets: [
    [
      'classic',
      {
        docs: {
          path: '../docs',
          routeBasePath: 'docs',
          sidebarPath: require.resolve('./sidebars.js'),
          exclude: ['SDK_POSITIONING.md', 'SKILLOPT_LITE.md'],
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
  themes: ['@docusaurus/theme-mermaid'],
  plugins: [
    [
      'docusaurus-pagefind-search',
      {
        rootSelector: 'main',
      },
    ],
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      image: 'img/aidememo-social-card.png',
      metadata: [
        {
          name: 'description',
          content:
            'AideMemo gives coding agents local, typed project memory through one Rust binary, MCP, CLI, and a Python agent SDK.',
        },
        {
          name: 'keywords',
          content:
            'coding agent memory, MCP memory, local-first AI, agent SDK, Rust, SQLite, knowledge graph',
        },
        {
          name: 'theme-color',
          content: '#071718',
        },
        {
          name: 'twitter:card',
          content: 'summary_large_image',
        },
      ],
      navbar: {
        title: 'AideMemo',
        logo: {
          alt: 'AideMemo logo',
          src: 'img/aidememo-logo.png',
        },
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
          {
            type: 'localeDropdown',
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
                label: 'Overview',
                to: '/docs/INTRODUCTION',
              },
              {
                label: 'Architecture',
                to: '/docs/ARCHITECTURE',
              },
              {
                label: 'Quickstart',
                to: '/docs/QUICKSTART',
              },
              {
                label: 'Agent Workflows',
                to: '/docs/AGENT_WORKFLOWS',
              },
              {
                label: 'MCP Setup',
                to: '/docs/MCP',
              },
              {
                label: 'Coding Agent Setup',
                to: '/docs/CODING_AGENTS',
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
