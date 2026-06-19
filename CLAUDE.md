@AGENTS.md

## mimo / sprint model selection

When a GPT (OpenAI) model is selected for mimo — including the `sprint` and
`mimo-code` executors — always pin `venice/openai-gpt-55`. The direct `openai/*`
ids (codex and gpt) fail under our ChatGPT-account auth ("not supported when
using Codex with a ChatGPT account"); `venice/openai-gpt-55` is the latest
non-pro OpenAI GPT and works via Venice auth.
