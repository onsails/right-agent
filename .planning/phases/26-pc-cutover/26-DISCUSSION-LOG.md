# Phase 26: PC Cutover - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-01
**Phase:** 26-pc-cutover
**Areas discussed:** CC agent process command, Channels cleanup scope, deleteWebhook failure mode, Doctor webhook check impl

---

## CC agent process command

| Option | Description | Selected |
|--------|-------------|----------|
| Только bot entries — убрать CC entry | Phase 26 убирает CC interactive process entries полностью. Bot process IS the agent process. | ✓ |
| Инлайн claude invocation | process_compose.rs генерирует команду `claude ...` напрямую в YAML | |
| Новый rightclaw agent subcommand | rightclaw-cli получает `rightclaw agent --agent <name>` который exec-ает claude | |

**User's choice:** Только bot entries — CC interactive process entries убираются.
**Notes:** Пользователь спросил "а зачем его пустым оставлять, может просто удалить?" — логика ясная. С бот-архитектурой нет смысла держать persistent CC session в process-compose. Каждое сообщение = свой `claude -p`.

---

## Channels cleanup scope

| Option | Description | Selected |
|--------|-------------|----------|
| Убрать вызовы, не трогать файлы | Удалить 3 вызова из cmd_up. Старые `.claude/channels/telegram/` остаются на диске. | ✓ |
| Убрать вызовы + активно чистить директорию | cmd_up активно удаляет `.claude/channels/telegram/` при каждом `rightclaw up`. | |

**User's choice:** "ваще забей. мы не в проде. просто не создавай их снова"
**Notes:** Non-production environment — достаточно перестать создавать новые channel config файлы.

---

## deleteWebhook failure mode

| Option | Description | Selected |
|--------|-------------|----------|
| fatal error | Err propagates, process-compose restarts bot. | ✓ |
| log + continue | warn в tracing и продолжить старт. | |

**User's choice:** fatal error
**Notes:** Если webhook не удалён — long-polling конкурирует с webhook, сообщения дропаются. Hard fail = корректное поведение.

---

## Doctor webhook check impl

| Option | Description | Selected |
|--------|-------------|----------|
| reqwest в doctor.rs | GET api.telegram.org/bot<token>/getWebhookInfo. Без новых зависимостей. | ✓ |
| Добавить teloxide в rightclaw | Bot::new(token).get_webhook_info(). Тяжёлый фреймворк в core крейте. | |

**User's choice:** reqwest (после обсуждения архитектуры)
**Notes:** Пользователь предложил `telegram-core` крейт для решения проблемы разделения зависимостей — хорошая идея, отложена в deferred. Для Phase 26 reqwest достаточен (уже есть в rightclaw deps).

---

## Claude's Discretion

- Разрешение `current_exe()` внутри `generate_process_compose` или снаружи
- Формат env var (RC_TELEGRAM_TOKEN vs RC_TELEGRAM_TOKEN_FILE) — следовать существующему приоритету
- Структура тестов для переписанного process_compose_tests.rs

## Deferred Ideas

- `telegram-core` crate — выделить Telegram утилиты в отдельный крейт для переиспользования между `rightclaw` и `bot`
- PROMPT-03 — `shell_wrapper.rs` не существует, но помечена pending; Phase 26 фактически закрывает её
