# RightClaw

**Tagline**
A multi-agent runtime for Claude Code — independent agents, own identities, OpenShell enforces.

## What It Is

RightClaw — это multi-agent runtime поверх Claude Code и NVIDIA OpenShell. Каждый агент — это **отдельная Claude Code сессия** со своим identity (IDENTITY.md), памятью (MEMORY.md), набором skills, tools, MCP-серверов и OpenShell policy. Агенты работают независимо друг от друга — нет мастер-сессии, нет оркестратора. Каждый агент автономен и отвечает за свою зону.

General-purpose по охвату (от рисёрча и коммуникаций до автоматизации рабочих процессов), но с сильным техническим ядром. CronSync (`/loop 5m`, reconciles YAML-спеки из `crons/` с живыми cron job'ами) запускает цепочки автономно. ClawHub skills подключаются через policy gate с автоматическим аудитом.

В отличие от OpenClaw (широкий доступ к системе, проблемы с безопасностью, CVE, юридические претензии от Anthropic), RightClaw делает ставку на правильный подход: использует только официальные механизмы Claude Code (skills, hooks, /loop, /schedule, MCP) и запускает всё в изолированном окружении с декларативными YAML-политиками OpenShell.

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

Каждый агент — независимая Claude Code сессия. Нет центрального оркестратора. Агенты запускаются параллельно через `start.sh`, каждый в своём sandbox.

```
┌──────────────────────────────────────────────────────────────────┐
│                        NVIDIA OpenShell                          │
│                                                                  │
│  ┌────────────────┐  ┌────────────────┐  ┌────────────────┐     │
│  │  Agent: watch  │  │  Agent: review │  │  Agent: ops    │ ... │
│  │  (CC session)  │  │  (CC session)  │  │  (CC session)  │     │
│  │                │  │                │  │                │     │
│  │ IDENTITY.md    │  │ IDENTITY.md    │  │ IDENTITY.md    │     │
│  │ MEMORY.md      │  │ MEMORY.md      │  │ MEMORY.md      │     │
│  │ skills: [...]  │  │ skills: [...]  │  │ skills: [...]  │     │
│  │ tools:  [...]  │  │ tools:  [...]  │  │ tools:  [...]  │     │
│  │ MCP:   [gh]    │  │ MCP:   [gh]    │  │ MCP: [slack]   │     │
│  │ crons: [...]   │  │ crons: [...]   │  │ crons: [...]   │     │
│  │                │  │                │  │                │     │
│  │ ┌────────────┐ │  │ ┌────────────┐ │  │ ┌────────────┐ │     │
│  │ │  policy:   │ │  │ │  policy:   │ │  │ │  policy:   │ │     │
│  │ │  net: gh   │ │  │ │  fs: r/o   │ │  │ │  net: *    │ │     │
│  │ │  fs: r/o   │ │  │ │  net: gh   │ │  │ │  fs: r/w   │ │     │
│  │ └────────────┘ │  │ └────────────┘ │  │ └────────────┘ │     │
│  └────────────────┘  └────────────────┘  └────────────────┘     │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                OpenShell Policy Engine                     │  │
│  │        filesystem │ network │ process │ inference          │  │
│  └────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘
```

### Память агентов

Каждый агент имеет свой `MEMORY.md` — файл, в который агент записывает важную информацию между сессиями. Claude Code нативно читает и пишет в этот файл.

**Фаза 1 (MVP):** per-agent `MEMORY.md`, без shared memory между агентами.

**Фаза 2:** shared memory через MCP memory server (SQLite или knowledge graph). Все агенты подключаются к одному серверу, пишут tagged записи (`source: reviewer`, `source: watchdog`), читают всё что релевантно.

## Scheduled Tasks — CronSync

CronSync — reconciliation skill. Каждый агент запускает свой CronSync, который синхронизирует желаемое состояние (YAML-спеки в `agents/<name>/crons/`) с фактическим (живые cron job'ы в сессии этого агента).

### Примитивы Claude Code

Claude Code предоставляет три tool'а для cron job'ов:

- **CronCreate** — создаёт job (5-field cron expression + prompt). Возвращает job ID.
- **CronList** — список активных job'ов с ID, расписанием, промптом.
- **CronDelete** — удаляет job по ID.

CronUpdate нет. Для изменения расписания или промпта — delete + create.

Job'ы живут в сессии. Auto-expire через 3 дня. Макс 50 на сессию.

### Как работает CronSync

Запускается вручную или по `/loop`:

```
/loop 5m /cronsync
```

Каждый тик:

1. Читает `crons/*.yaml` → **desired state**
2. Вызывает `CronList` → **actual state**
3. Сопоставляет через `crons/state.json` (маппинг имя файла ↔ job ID)
4. Reconcile:
   - Спека есть, job'а нет → `CronCreate`, записать ID в state
   - Job есть, спеки нет → `CronDelete`, удалить из state
   - Спека изменилась (schedule или prompt) → `CronDelete` + `CronCreate`, обновить ID в state
   - Совпадает → skip

### Формат спеки

```yaml
# crons/deploy-check.yaml
schedule: "*/5 * * * *"
lock_ttl: 10m
max_turns: 5
prompt: "Check CI status for all open PRs, post comment if broken"
```

```yaml
# crons/morning-briefing.yaml
schedule: "0 9 * * 1-5"
lock_ttl: 30m
prompt: "Gather open PRs, failing tests, pending reviews. Post summary to Slack."
```

- `schedule` — стандартное 5-field cron expression (то, что CronCreate принимает). Для простых интервалов — cron-синтаксис: `*/5 * * * *` (каждые 5 мин), `0 */2 * * *` (каждые 2 часа).
- `lock_ttl` — максимальное время жизни lock'а (default: 30m). После этого lock считается stale.
- `max_turns` — опционально, ограничивает количество tool-use turns в сессии крона (передаётся как `--max-turns` в Claude CLI). Полезно для коротких проверок, чтобы агент не уходил в бесконечный цикл. Default: без ограничения.

### Concurrency Control

Проблема: cron стреляет каждые 5 минут, но предыдущий запуск может ещё работать. Нативного "is agent X still running?" в Claude Code нет.

Решение — **lock-файлы с heartbeat**. CronSync оборачивает промпт каждого крона guard-логикой:

1. Проверить `crons/.locks/{name}.json`
   - Есть и `heartbeat` < `lock_ttl` назад → **skip**, предыдущий запуск ещё работает
   - Есть и `heartbeat` > `lock_ttl` назад → **stale lock**, удалить
   - Нет → продолжить
2. Создать lock-файл с текущим heartbeat
3. Выполнить prompt из спеки
4. Периодически обновлять heartbeat в lock-файле
5. По завершении — удалить lock-файл

Lock-файл:
```json
{"heartbeat": "2026-03-21T10:05:00Z"}
```

Все таймстампы строго **UTC ISO 8601** (суффикс `Z`).

### State

```json
// crons/state.json
{
  "deploy-check": {
    "job_id": "4e9fed67",
    "schedule": "*/5 * * * *",
    "prompt_hash": "a1b2c3d4"
  },
  "morning-briefing": {
    "job_id": "06c25e84",
    "schedule": "0 9 * * 1-5",
    "prompt_hash": "e5f6g7h8"
  }
}
```

`prompt_hash` — для детекта изменений в промпте без хранения полного текста.

### Зачем CronSync, а не ручные `/loop`

- Декларативно: добавить задачу = создать YAML. Удалить = удалить файл. Изменить расписание = отредактировать YAML. CronSync подхватит на следующем тике.
- Версионируемо: `crons/` в git, PR-ревью на изменения расписаний.
- Идемпотентно: CronSync можно запускать сколько угодно раз — если всё синхронизировано, ничего не делает.
- Восстановление: после рестарта сессии все job'ы пропадают. CronSync пересоздаёт их из спек.

### Структура

Каждый агент имеет свою директорию `crons/`:

```
agents/watchdog/crons/
├── deploy-check.yaml
├── ci-status.yaml
├── state.json              # gitignore — session-specific
└── .locks/                 # gitignore — runtime lock files
    ├── deploy-check.json
    └── ci-status.json

agents/ops/crons/
├── morning-briefing.yaml
├── dependency-audit.yaml
├── state.json
└── .locks/
    └── ...
```

## Agents (v1 — MVP)

Каждый agent — отдельная Claude Code сессия со своим identity, памятью и набором skills/tools. Агенты запускаются через `rightclaw up`.

### 1. watchdog — Мониторинг и алерты
* Автономная сессия с scheduled tasks: проверяет деплой / CI / сервисы
* Анализирует логи, формулирует actionable summary
* OpenShell policy: доступ только к определённым API endpoints (GitHub Actions, Vercel, etc.)
* MCP: GitHub, Slack (для алертов)

### 2. reviewer — Автономное код-ревью
* Триггерится по cron на новые PR
* Read-only доступ к файлам, git log
* Skills: code-review conventions, checklist, severity levels
* Результат: комментарий в PR через MCP GitHub connector

### 3. scout — Разведка и due diligence
* Быстрый анализ репозиториев: архитектура, зависимости, лицензии, code quality
* Генерирует структурированный отчёт
* Policy: read-only доступ к целевому репо, запрет на запись и внешние сетевые вызовы

### 4. ops — Рутинные операции
* Morning briefing: scheduled task, собирает статус проектов, непрочитанные PR, failing tests
* Changelog generator: hook на git push, автоматический draft release notes
* Dependency auditor: периодическая проверка уязвимостей
* MCP: GitHub, Slack, Linear

### 5. forge — Scaffolding новых проектов
* Генерация проекта по PRD (Rust, TypeScript, Zola)
* Создаёт структуру, конфиги, CI/CD pipeline
* Включает шаблон OpenShell policy для нового проекта

## Запуск — CLI + process-compose

RightClaw CLI — тонкая обёртка над [process-compose](https://github.com/F1bonacc1/process-compose). CLI читает `agents/`, генерирует `process-compose.yaml` в `/tmp/rightclaw/`, запускает process-compose. Для юзера — одна команда.

### UX

```bash
# Запуск всех агентов (TUI открывается сразу)
rightclaw up ~/my-project

# Только конкретные агенты
rightclaw up ~/my-project --agents watchdog,reviewer

# В фоне (TUI сервер запущен, но терминал свободен)
rightclaw up ~/my-project -d

# Подключиться к TUI запущенных агентов (из любого терминала)
rightclaw attach

# Статус
rightclaw status

# Рестарт одного агента
rightclaw restart reviewer

# Остановить всё
rightclaw down
```

### Что делает `rightclaw up`

1. Сканирует `agents/` — каждая поддиректория с `IDENTITY.md` = агент
2. Генерирует `/tmp/rightclaw/<hash>/process-compose.yaml`:

```yaml
# Генерируется автоматически, не редактировать
version: "0.5"

processes:
  watchdog:
    command: >
      claude --dangerously-skip-permissions
        --append-system-prompt-file /path/to/rightclaw/agents/watchdog/IDENTITY.md
        -p /home/user/my-project
        --prompt "You are starting. Read your MEMORY.md and crons/ to restore context."
    working_dir: /home/user/my-project
    availability:
      restart: "on_failure"
      max_restarts: 5
      backoff_seconds: 10

  reviewer:
    command: >
      claude --dangerously-skip-permissions
        --append-system-prompt-file /path/to/rightclaw/agents/reviewer/IDENTITY.md
        -p /home/user/my-project
        --prompt "You are starting. Read your MEMORY.md and crons/ to restore context."
    working_dir: /home/user/my-project
    availability:
      restart: "on_failure"
      max_restarts: 5
      backoff_seconds: 10
```

3. Запускает process-compose:
   - `rightclaw up` → `process-compose up -f <yaml>` (TUI сразу)
   - `rightclaw up -d` → `process-compose up -f <yaml> --detached-with-tui` (фон, TUI-сервер жив)
4. `rightclaw attach` → `process-compose attach` (подключение к TUI из любого терминала)
5. TUI показывает все агенты: статус, логи, uptime, restart count. `Ctrl+R` рестарт, `F9` стоп

### Конфигурация агентов

Каждый агент может иметь опциональный `agent.yaml` для настроек, специфичных для process-compose:

```yaml
# agents/watchdog/agent.yaml
restart: "always"          # default: "on_failure"
max_restarts: 0            # default: 5 (0 = unlimited)
backoff_seconds: 30        # default: 10
start_prompt: "Custom startup prompt for this agent"
```

Если `agent.yaml` нет — используются дефолты.

### Реализация CLI

Простой bash-скрипт или Go binary (TBD). MVP — bash. Зависимости: `process-compose`, `claude` (Claude Code CLI).

TODO: обернуть в `openshell sandbox create --policy ...` когда OpenShell будет доступен.

### Структура репозитория

```
rightclaw/
├── rightclaw              # CLI (bash-скрипт)
├── agents/                 # определения агентов
│   ├── watchdog/
│   │   ├── IDENTITY.md     # identity агента (system prompt)
│   │   ├── MEMORY.md       # персистентная память агента
│   │   ├── agent.yaml      # опционально: restart policy, backoff, etc.
│   │   ├── skills/         # skills этого агента
│   │   ├── crons/          # scheduled tasks этого агента
│   │   └── .mcp.json       # MCP-серверы этого агента
│   ├── reviewer/
│   │   ├── IDENTITY.md
│   │   ├── MEMORY.md
│   │   ├── agent.yaml
│   │   ├── skills/
│   │   ├── crons/
│   │   └── .mcp.json
│   ├── scout/
│   │   └── ...
│   ├── ops/
│   │   └── ...
│   └── forge/
│       └── ...
├── shared/                 # общие ресурсы для всех агентов
│   └── skills/
│       └── clawhub/
│           └── SKILL.md    # /clawhub — менеджер скиллов
├── policies/               # OpenShell policies
│   ├── watchdog.yaml
│   ├── reviewer.yaml
│   └── ...
├── CLAUDE.md               # dev-инструкции (для разработки самого rightclaw)
└── seed.md                 # этот документ
```

## Что поставляется

Каждый agent — самодостаточная директория с identity, памятью, skills, crons и MCP-конфигом. Агенты также могут публиковаться как skill packs в ClawHub:

```
rightclaw/
├── README.md
├── agents/
│   ├── watchdog/
│   │   ├── IDENTITY.md         # кто этот агент, как себя ведёт
│   │   ├── MEMORY.md           # персистентная память
│   │   ├── agent.yaml          # restart policy, backoff, start prompt
│   │   ├── skills/             # skills этого агента
│   │   │   └── log-analyzer/
│   │   │       └── SKILL.md
│   │   ├── crons/              # scheduled tasks
│   │   │   └── deploy-check.yaml
│   │   ├── hooks/              # lifecycle hooks
│   │   └── .mcp.json           # MCP-серверы
│   ├── reviewer/
│   │   └── ...
│   ├── scout/
│   │   └── ...
│   ├── ops/
│   │   └── ...
│   └── forge/
│       └── ...
├── policies/                   # OpenShell YAML policies
│   ├── watchdog.yaml
│   └── reviewer.yaml
└── shared/
    └── skills/                 # skills, доступные всем агентам
        └── clawhub/
            └── SKILL.md
```

## Установка (целевой UX)

Никакого CLI. Всё через промпты — пользователь пишет в Telegram-канал или напрямую в Claude Code:

```
установи скилл onsails/rightclaw-reviewer
```

Claude вызывает skill `/clawhub` → ищет в каталоге → клонит → кладёт в `.claude/skills/` → проверяет через policy gate.

Или напрямую из git (без ClawHub):
```
склонируй https://github.com/onsails/rightclaw в ~/.claude/skills/rightclaw
```

## ClawHub

ClawHub — веб-каталог скиллов для Claude Code (3000+ skills, semver, vector search). Не CLI, не пакетный менеджер — реестр с API.

Для работы с ClawHub в RightClaw есть **встроенный skill `/clawhub`** (см. `skills/clawhub/SKILL.md`). Claude вызывает его когда пользователь хочет:
- Найти скилл: `найди скилл для код-ревью`
- Установить: `установи TheSethRose/agent-browser`
- Удалить: `удали скилл agent-browser`
- Список установленных: `какие скиллы установлены?`

### Что делает `/clawhub install`

1. Ищет скилл в каталоге ClawHub (по имени или vector search по описанию)
2. Клонит git-репо скилла в `.claude/skills/{name}/`
3. **Policy gate** — анализирует SKILL.md frontmatter (required binaries, env vars, network access) и генерирует минимально необходимую OpenShell policy. Подозрительные паттерны (exfiltration, broad filesystem access, неизвестные бинарники) блокируются или требуют подтверждения пользователя.
4. Регистрирует скилл в `skills/installed.json`

### Совместимость

**RightClaw → ClawHub.** RightClaw skill packs публикуются в ClawHub как стандартные AgentSkills bundle (SKILL.md + YAML frontmatter). OpenShell policies поставляются как доп. файлы — игнорируются вне OpenShell, активируются автоматически внутри sandbox.

**ClawHub → RightClaw.** Любой ClawHub skill устанавливается через `/clawhub install`. Разница с обычной установкой — policy gate перед активацией.

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

Расширение `openshell` прозрачно для ClawHub (opaque metadata), но распознаётся RightClaw для автоматической настройки sandbox.

## Позиционирование

| | OpenClaw | RightClaw |
|---|---|---|
| Runtime | Bare metal / Docker | NVIDIA OpenShell sandbox |
| Модель доступа | Любые токены (включая arbitrage) | Только Claude API / легитимная подписка |
| Безопасность | Широкий доступ по умолчанию | Declarative YAML policies, principle of least privilege |
| Экосистема | Plugins (risk of malicious skills) | ClawHub-compatible + policy gate для сторонних skills |
| Отношения с Anthropic | Cease & desist, ребрендинги | Полный compliance, использует только официальные механизмы |
| Архитектура | Monolithic agent | Multi-agent (каждый агент — своя CC сессия) |
| Фокус | General-purpose personal AI agent | General-purpose, но с сильным техническим ядром |

## Название

RightClaw = делаем claw (агента) правильно. Правая клешня — точная, хирургическая, в отличие от левой (OpenClaw), которая хватает всё подряд. Также аллюзия на "right way" — правильный путь работы с Claude Code.

### Бренд-связь с onsails

RightClaw — продукт студии onsails. Навигация требует точных инструментов: правильный курс, правильный ветер, правильная клешня. 🦞⛵
