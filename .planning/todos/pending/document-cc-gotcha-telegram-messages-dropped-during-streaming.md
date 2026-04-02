---
id: document-cc-gotcha-telegram-messages-dropped-during-streaming
title: Document CC gotcha — Telegram messages dropped while agent is streaming
area: docs
status: pending
priority: medium
created: 2026-03-28
---

## Problem

CC silently drops Telegram channel messages that arrive while it is actively generating a response (streaming). No error, no queuing — the message is consumed by the Telegram bun plugin's polling loop but CC never fires a channel notification for it.

This causes users to think their message was ignored when they reply before the agent has finished responding.

## Solution

Add to CC gotchas in memory and/or CLAUDE.md:

> "CC channel messages (Telegram) are silently dropped if they arrive while CC is actively streaming a response. Users must wait for the typing indicator to stop before sending the next message. There is no queuing or retry — the message is permanently lost."

Consider also adding a note in rightclaw docs/IDENTITY.md template to set user expectations.

## Files

- `/home/wb/.claude/projects/-home-wb-dev-rightclaw/memory/MEMORY.md` — add to CC Gotchas section
