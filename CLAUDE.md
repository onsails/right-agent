@AGENTS.md

## mimo / sprint model selection

When a GPT (OpenAI) model is selected for mimo — including the `sprint` and
`mimo-code` executors — always pin `venice/openai-gpt-55`. The direct `openai/*`
ids (codex and gpt) fail under our ChatGPT-account auth ("not supported when
using Codex with a ChatGPT account"); `venice/openai-gpt-55` is the latest
non-pro OpenAI GPT and works via Venice auth.

## Worktree site dev server — symlink `.env.local`

When running the `site/` dev server from a git **worktree**, always symlink the
main checkout's gitignored `site/.env.local` into the worktree first:
`ln -s <main>/site/.env.local <worktree>/site/.env.local`. `git worktree` does
not carry untracked/gitignored files, and `site/astro.config.mjs` reads
`RIGHT_SITE_DEV_ALLOWED_HOSTS` (dev `vite.server.allowedHosts`, e.g. the
tailnet host) from `.env.local`. Without the symlink the worktree dev server
omits `allowedHosts` and Vite blocks remote/tailnet hosts. Restart the dev
server after linking (astro reads `.env.local` only at startup).
