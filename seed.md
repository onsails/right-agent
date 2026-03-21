# RightClaw

**Tagline**
A sandboxed agent runtime for Claude Code — master session orchestrates, subagents execute, OpenShell enforces.

## What It Is

RightClaw — это pre-configured agent runtime поверх Claude Code и NVIDIA OpenShell. Мастер-сессия Claude Code запускается внутри OpenShell sandbox и выступает оркестратором: принимает задачи, планирует выполнение, делегирует работу специализированным субагентам. Каждый субагент работает со своим набором skills, tools и отдельной OpenShell policy — ровно те права, которые нужны для конкретной задачи, и ни байтом больше.

General-purpose по охвату (от рисёрча и коммуникаций до автоматизации рабочих процессов), но с сильным техническим ядром. CronMaster (один `/loop 1m`, читает YAML-задачи из `crons/`) запускает цепочки автономно. ClawHub skills подключаются через policy gate с автоматическим аудитом.

В отличие от OpenClaw (широкий доступ к системе, проблемы с безопасностью, CVE, юридические претензии от Anthropic), RightClaw делает ставку на правильный подход: использует только официальные механизмы Claude Code (skills, subagents, hooks, /loop, /schedule, MCP) и запускает всё в изолированном окружении с декларативными YAML-политиками OpenShell.

## Кому это нужно

* Разработчики-одиночки и небольшие команды, которые хотят автоматизировать рутину (мониторинг, код-ревью, PR-создание, тесты, рисёрч) без развёртывания отдельной инфраструктуры.
* Технические консультанты и Angel-Operators, которым нужна воспроизводимая среда для быстрого аудита и due diligence проектов.
* Продвинутые пользователи, которым нужен personal AI agent уровня OpenClaw, но без security-кошмаров — с прозрачными политиками и sandbox-изоляцией из коробки.

## Ключевые принципы

1. **Security-first** — каждый skill pack поставляется с OpenShell policy (YAML), которая ограничивает файловую систему, сеть, процессы и inference-маршруты. Никакого "grant all, pray it works".
2. **Официальный путь** — только Claude API или легитимная подписка. Никакого token arbitrage, никаких внутренних API. RightClaw — это антитеза OpenClaw по compliance.
3. **Composable** — skills комбинируются как LEGO. Subagent для код-ревью может вызвать skill для линтинга, который триггерит hook для автоформатирования. Scheduled task запускает цепочку каждое утро.
4. **Batteries included, but removable** — каждый pack самодостаточен. Не нужен весь набор, чтобы использовать один skill.
5. **ClawHub-compatible** — RightClaw skills публикуются в ClawHub (реестр с 3000+ skills, semver, vector search). Любой skill из ClawHub можно установить в RightClaw-окружение, но он проходит через policy gate — автоматический аудит разрешений перед активацией в sandbox.

## Архитектура (высокий уровень)

```
┌──────────────────────────────────────────────────────────────┐
│                      NVIDIA OpenShell                        │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │            Master Session (Claude Code)                │  │
│  │                                                        │  │
│  │  Orchestrator: принимает задачи, планирует,            │  │
│  │  делегирует субагентам, агрегирует результаты          │  │
│  │                                                        │  │
│  │  ┌──────────────────────┐  ┌──────────┐               │  │
│  │  │ CronMaster           │  │ ClawHub  │               │  │
│  │  │ /loop 1m → crons/*.y │  │ (import) │               │  │
│  │  └──────────┬───────────┘  └────┬─────┘               │  │
│  │             └───────────────────┘                      │  │
│  │                      ▼                                 │  │
│  │              Task Delegation                           │  │
│  └──────────┬───────────┼───────────┬─────────────────────┘  │
│             │           │           │                        │
│             ▼           ▼           ▼                        │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐         │
│  │  Subagent A  │ │  Subagent B  │ │  Subagent C  │  ...    │
│  │  (reviewer)  │ │  (scout)     │ │  (ops)       │         │
│  │              │ │              │ │              │         │
│  │ skills: [...] │ │ skills: [...] │ │ skills: [...] │         │
│  │ tools:  [...] │ │ tools:  [...] │ │ tools:  [...] │         │
│  │ MCP:   [gh]  │ │ MCP:   []    │ │ MCP: [slack] │         │
│  │              │ │              │ │              │         │
│  │ ┌──────────┐ │ │ ┌──────────┐ │ │ ┌──────────┐ │         │
│  │ │ policy:  │ │ │ │ policy:  │ │ │ │ policy:  │ │         │
│  │ │ net: gh  │ │ │ │ fs: r/o  │ │ │ │ net: *   │ │         │
│  │ │ fs: r/o  │ │ │ │ net: off │ │ │ │ fs: r/w  │ │         │
│  │ └──────────┘ │ │ └──────────┘ │ │ └──────────┘ │         │
│  └──────────────┘ └──────────────┘ └──────────────┘         │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │              OpenShell Policy Engine                   │  │
│  │      filesystem │ network │ process │ inference        │  │
│  └────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────┘
```

## Scheduled Tasks — CronMaster

Все запланированные задачи управляются одним процессом — CronMaster. Это skill, который запускается через `/loop 1m` и работает как минималистичный cron-демон внутри Claude Code.

### Как работает

1. CronMaster запускается один раз: `/loop 1m /cronmaster`
2. Каждую минуту он:
   - Читает все файлы из `crons/` (YAML)
   - Читает `crons/state.json` (last_run для каждой задачи)
   - Для каждой задачи проверяет: пора запускать?
   - Если пора — спавнит subagent с промптом из задачи
   - Обновляет `state.json`

### Формат задачи

Одно поле `schedule` — принимает и интервалы, и cron-выражения:

```yaml
# crons/deploy-check.yaml
schedule: 5m
prompt: "Check CI status for all open PRs, post comment if broken"
agent: watchdog
```

```yaml
# crons/morning-briefing.yaml
schedule: "0 9 * * 1-5"
prompt: "Gather open PRs, failing tests, pending reviews. Post summary to Slack."
agent: ops
```

`schedule` — единственное поле для расписания. CronMaster сам определяет формат:
- Содержит единицу времени (`5m`, `1h`, `30s`) → интервал, сравнивает с `last_run`
- Иначе → cron-выражение, проверяет матч с текущим временем

### State

```json
// crons/state.json
{
  "deploy-check": {
    "last_run": "2026-03-21T10:05:00Z",
    "last_status": "ok"
  },
  "morning-briefing": {
    "last_run": "2026-03-21T09:00:00Z",
    "last_status": "ok"
  }
}
```

### Почему один `/loop`, а не по одному на задачу

- `/loop 5m /deploy-check` + `/loop "0 9 * * 1-5" /morning-briefing` + ... = N параллельных loop-процессов, каждый держит контекст
- `/loop 1m /cronmaster` = один процесс, читает YAML, спавнит subagent'ов по необходимости
- Добавить задачу = создать YAML-файл. Удалить = удалить файл. Без перезапуска loop'ов.

### Структура

```
crons/
├── deploy-check.yaml
├── morning-briefing.yaml
├── dependency-audit.yaml
└── state.json
```

## Skill Packs (v1 — MVP)

### 1. rightclaw/watchdog — Мониторинг и алерты
* Scheduled task: проверяет состояние деплоя / CI / сервисов по расписанию
* Subagent: анализирует логи, формулирует actionable summary
* OpenShell policy: доступ только к определённым API endpoints (GitHub Actions, Vercel, etc.)

### 2. rightclaw/reviewer — Автономное код-ревью
* Триггерится по /loop или schedule на новые PR
* Subagent с ограниченными tools (read-only доступ к файлам, git log)
* Skill: code-review conventions, checklist, severity levels
* Результат: комментарий в PR через MCP GitHub connector

### 3. rightclaw/scout — Разведка и due diligence
* Skill pack для быстрого анализа репозиториев: архитектура, зависимости, лицензии, code quality
* Subagent: генерирует структурированный отчёт
* Policy: read-only доступ к целевому репо, запрет на запись и внешние сетевые вызовы

### 4. rightclaw/ops — Рутинные операции
* Morning briefing: scheduled task, который собирает статус проектов, непрочитанные PR, failing tests
* Changelog generator: hook на git push, автоматический draft release notes
* Dependency auditor: периодическая проверка уязвимостей

### 5. rightclaw/forge — Scaffolding новых проектов
* Skill: генерация проекта по PRD (Rust, TypeScript, Zola)
* Subagent: создаёт структуру, конфиги, CI/CD pipeline
* Включает шаблон OpenShell policy для нового проекта

## Что поставляется

Каждый skill pack — это директория, которая копируется в `.claude/skills/` или `~/.claude/skills/`:

```
rightclaw/
├── README.md
├── watchdog/
│   ├── SKILL.md              # основной skill с YAML frontmatter
│   ├── agents/               # subagent definitions
│   │   └── log-analyzer.md
│   ├── hooks/                # lifecycle hooks
│   ├── policies/             # OpenShell YAML policies
│   │   └── watchdog.yaml
│   └── scheduled/            # scheduled task templates
│       └── deploy-check.md
├── reviewer/
│   └── ...
├── scout/
│   └── ...
├── ops/
│   └── ...
└── forge/
    └── ...
```

## Установка (целевой UX)

```bash
# Внутри OpenShell sandbox
openshell sandbox create -- claude

# Установка всех packs через ClawHub
clawhub install onsails/rightclaw

# Или отдельный pack
clawhub install onsails/rightclaw-reviewer

# Или напрямую из git (без ClawHub)
git clone https://github.com/onsails/rightclaw ~/.claude/skills/rightclaw

# Применить OpenShell policy для pack'а
openshell policy set <sandbox> --policy ~/.claude/skills/rightclaw/reviewer/policies/reviewer.yaml

# Установить любой skill из ClawHub в RightClaw-окружение
# Policy gate автоматически проверит его перед активацией
clawhub install TheSethRose/agent-browser
# → RightClaw policy gate: "agent-browser requires network access to *.
#    Generate restrictive OpenShell policy? [Y/n]"
```

## Совместимость с ClawHub

**RightClaw → ClawHub (публикация).** Каждый RightClaw skill pack публикуется в ClawHub как стандартный AgentSkills bundle (SKILL.md с YAML frontmatter, semver, changelogs). Любой пользователь OpenClaw или другого агента может установить RightClaw skills через `clawhub install`. OpenShell policies поставляются как дополнительные файлы — они игнорируются вне OpenShell, но активируются автоматически внутри sandbox.

**ClawHub → RightClaw (импорт с аудитом).** Любой из 3000+ skills ClawHub можно установить в RightClaw-окружение. Разница: перед активацией skill проходит через policy gate — встроенный subagent, который анализирует SKILL.md frontmatter (required binaries, env vars, network access) и генерирует минимально необходимую OpenShell policy. Подозрительные паттерны (exfiltration, broad filesystem access, неизвестные бинарники) блокируются или требуют явного подтверждения.

**Формат.** RightClaw skills используют стандартный ClawHub SKILL.md формат + расширение `openshell` в metadata frontmatter:

```yaml
---
name: rightclaw-reviewer
description: Autonomous code review with policy-enforced sandbox
version: 1.0.0
metadata:
  openshell:
    policy: policies/reviewer.yaml
    required_mcp: [github]
    filesystem: read-only
    network: [api.github.com]
---
```

Это расширение прозрачно для ClawHub (хранится как opaque metadata), но распознаётся RightClaw для автоматической настройки sandbox.

## Позиционирование

| | OpenClaw | RightClaw |
|---|---|---|
| Runtime | Bare metal / Docker | NVIDIA OpenShell sandbox |
| Модель доступа | Любые токены (включая arbitrage) | Только Claude API / легитимная подписка |
| Безопасность | Широкий доступ по умолчанию | Declarative YAML policies, principle of least privilege |
| Экосистема | Plugins (risk of malicious skills) | ClawHub-compatible + policy gate для сторонних skills |
| Отношения с Anthropic | Cease & desist, ребрендинги | Полный compliance, использует только официальные механизмы |
| Фокус | General-purpose personal AI agent | General-purpose, но с сильным техническим ядром |

## Название

RightClaw = делаем claw (агента) правильно. Правая клешня — точная, хирургическая, в отличие от левой (OpenClaw), которая хватает всё подряд. Также аллюзия на "right way" — правильный путь работы с Claude Code.

### Бренд-связь с onsails

RightClaw — продукт студии onsails. Навигация требует точных инструментов: правильный курс, правильный ветер, правильная клешня. 🦞⛵
