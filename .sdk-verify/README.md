# Claude Agent SDK

![](https://img.shields.io/badge/Node.js-18%2B-brightgreen?style=flat-square) [![npm]](https://www.npmjs.com/package/@anthropic-ai/claude-agent-sdk)

[npm]: https://img.shields.io/npm/v/@anthropic-ai/claude-agent-sdk.svg?style=flat-square

The Claude Agent SDK enables you to programmatically build AI agents with Claude Code's capabilities. Create autonomous agents that can understand codebases, edit files, run commands, and execute complex workflows.

**Learn more in the [official documentation](https://platform.claude.com/docs/en/agent-sdk/overview)**.

## Get started

Install the Claude Agent SDK:

```sh
npm install @anthropic-ai/claude-agent-sdk
```

## Compiled binaries (`bun build --compile`)

When bundling your application into a single executable with `bun build --compile`, the SDK cannot resolve the native CLI binary at runtime — `require.resolve` doesn't work from inside the compiled `$bunfs` (or `B:\~BUN\...` on Windows) virtual filesystem.

Embed the platform-specific binary as a file asset, extract it to a real path, and pass it explicitly:

```js
import binPath from '@anthropic-ai/claude-agent-sdk-darwin-arm64/claude' with { type: 'file' }
import { extractFromBunfs } from '@anthropic-ai/claude-agent-sdk/extract'
import { query } from '@anthropic-ai/claude-agent-sdk'

const cliPath = extractFromBunfs(binPath)

for await (const message of query({
  prompt: '…',
  options: { pathToClaudeCodeExecutable: cliPath },
})) { /* … */ }
```

Each compiled executable embeds one platform's binary, matching your `--target`. Cross-compiling requires installing the non-matching platform package (e.g. `npm install @anthropic-ai/claude-agent-sdk-linux-x64 --force`). On Windows the binary subpath is `/claude.exe` (e.g. `@anthropic-ai/claude-agent-sdk-win32-x64/claude.exe`).

## Migrating from the Claude Code SDK

The Claude Code SDK is now the Claude Agent SDK. Please check out the [migration guide](https://platform.claude.com/docs/en/agent-sdk/migration-guide) for details on breaking changes.

## Reporting Bugs

We welcome your feedback. File a [GitHub issue](https://github.com/anthropics/claude-agent-sdk-typescript/issues) to report bugs or request features.

## Connect on Discord

Join the [Claude Developers Discord](https://anthropic.com/discord) to connect with other developers building with the Claude Agent SDK. Get help, share feedback, and discuss your projects with the community.

## Data collection, usage, and retention

When you use the Claude Agent SDK, we collect feedback, which includes usage data (such as code acceptance or rejections), associated conversation data, and user feedback submitted via the /bug command.

### How we use your data

See our [data usage policies](https://docs.anthropic.com/en/docs/claude-code/data-usage).

### Privacy safeguards

We have implemented several safeguards to protect your data, including limited retention periods for sensitive information and restricted access to user session data.

For full details, please review our [Commercial Terms of Service](https://www.anthropic.com/legal/commercial-terms) and [Privacy Policy](https://www.anthropic.com/legal/privacy).

