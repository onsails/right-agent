# RightClaw

**Tagline**
A sandboxed agent runtime for Claude Code — master session orchestrates, subagents execute, OpenShell enforces.

## What It Is

RightClaw — это pre-configured agent runtime поверх Claude Code и NVIDIA OpenShell. Мастер-сессия Claude Code запускается внутри OpenShell sandbox и выступает оркестратором: принимает задачи, планирует выполнение, делегирует работу специализированным субагентам. Каждый субагент работает со своим набором skills, tools и отдельной OpenShell policy — ровно те права, которые нужны для конкретной задачи, и ни байтом больше.

General-purpose по охвату (от рисёрча и коммуникаций до автоматизации рабочих процессов), но с сильным техническим ядром. CronSync (`/loop 5m`, reconciles YAML-спеки из `crons/` с живыми cron job'ами) запускает цепочки автономно. ClawHub skills подключаются через policy gate с автоматическим аудитом.

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
│  │  │ CronSync             │  │ ClawHub  │               │  │
│  │  │ /loop 5m → crons/*.y │  │ (import) │               │  │
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

## Scheduled Tasks — CronSync

CronSync — reconciliation skill. Синхронизирует желаемое состояние (YAML-спеки в `crons/`) с фактическим (живые cron job'ы в сессии Claude Code).

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
prompt: "Check CI status for all open PRs, post comment if broken"
```

```yaml
# crons/morning-briefing.yaml
schedule: "0 9 * * 1-5"
prompt: "Gather open PRs, failing tests, pending reviews. Post summary to Slack."
```

`schedule` — стандартное 5-field cron expression (то, что CronCreate принимает).
Для простых интервалов — cron-синтаксис: `*/5 * * * *` (каждые 5 мин), `0 */2 * * *` (каждые 2 часа).

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

```
crons/
├── deploy-check.yaml
├── morning-briefing.yaml
├── dependency-audit.yaml
└── state.json              # gitignore — session-specific
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
