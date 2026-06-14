// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sitemap from '@astrojs/sitemap';
import starlightLinksValidator from 'starlight-links-validator';

// GitHub Pages project page. Switch `site`/`base` (and add public/CNAME) for a custom domain.
export default defineConfig({
  site: 'https://onsails.github.io',
  base: '/right-agent',
  integrations: [
    starlight({
      title: 'right agent',
      // Docs live at src/content/docs/docs/* -> /docs/* (Starlight subpath pattern).
      customCss: [
        './src/styles/tokens.css',
        './src/styles/fonts.css',
        './src/styles/starlight.css',
      ],
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/onsails/right-agent' },
        { icon: 'telegram', label: 'Telegram', href: 'https://t.me/rightagent' },
      ],
      sidebar: [
        { label: 'Start', link: '/docs/' },
        { label: 'Install', link: '/docs/install/' },
        { label: 'Concepts', link: '/docs/concepts/' },
        { label: 'Security model', link: '/docs/security/' },
        { label: 'Telegram commands', link: '/docs/commands/' },
      ],
    }),
    sitemap(),
    starlightLinksValidator(),
  ],
});
