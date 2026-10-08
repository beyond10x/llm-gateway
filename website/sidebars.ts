import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docsSidebar: [
    {
      type: 'category',
      label: 'Start here',
      collapsed: false,
      items: ['index', 'getting-started', 'status'],
    },
    {
      type: 'category',
      label: 'Concepts',
      collapsed: false,
      items: ['concepts/single-owner-gateway', 'concepts/relay', 'concepts/hosting'],
    },
    {
      type: 'category',
      label: 'Guides',
      collapsed: false,
      items: ['guides/set-up-a-client', 'guides/scrape-the-counters', 'guides/run-the-checks'],
    },
    {
      type: 'category',
      label: 'Reference',
      collapsed: false,
      items: [
        'reference/cli',
        'reference/deployment-document',
        'reference/refusals',
        'reference/crates',
      ],
    },
  ],
};

export default sidebars;
