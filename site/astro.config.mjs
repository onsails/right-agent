// @ts-check
import { defineConfig } from 'astro/config';
import { loadEnv } from 'vite';
import starlight from '@astrojs/starlight';
import sitemap from '@astrojs/sitemap';
import starlightLinksValidator from 'starlight-links-validator';

// Dev-only allowed hosts (e.g. a Tailscale tailnet) come from a gitignored
// local file — RIGHT_SITE_DEV_ALLOWED_HOSTS in `.env.local`, comma-separated.
// Absent in prod/CI, so `vite.server.allowedHosts` is omitted entirely there.
const devAllowedHosts = loadEnv(process.env.NODE_ENV ?? '', process.cwd(), '')
  .RIGHT_SITE_DEV_ALLOWED_HOSTS?.split(',')
  .map((h) => h.trim())
  .filter(Boolean) ?? [];

// Custom apex domain on GitHub Pages. The domain lives in public/CNAME and the
// site serves from the root, so no `base` is set (defaults to '/').
export default defineConfig({
  site: 'https://right-agent.ai',
  ...(devAllowedHosts.length > 0 && {
    vite: { server: { allowedHosts: devAllowedHosts } },
  }),
  integrations: [
    starlight({
      title: 'right agent',
      // Docs-portal-only banner (overrides Starlight's frontmatter-only Banner so the
      // notice is global). Does not touch the landing.
      components: {
        Banner: './src/components/DocsBanner.astro',
        Head: './src/components/StarlightHead.astro',
      },
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
        { label: 'Self-evolution', link: '/docs/self-evolution/' },
        { label: 'Scheduled jobs', link: '/docs/scheduled-jobs/' },
        { label: 'Security model', link: '/docs/security/' },
        { label: 'Telegram commands', link: '/docs/commands/' },
      ],
    }),
    sitemap(),
    starlightLinksValidator(),
  ],
});
