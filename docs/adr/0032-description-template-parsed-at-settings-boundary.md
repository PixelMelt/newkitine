# 32. The description template is parsed at the settings boundary, not validated then rendered

Status: Accepted
Nicotine+: No counterpart

## Context

A template that can fail to render must not reach the peer-response hot path.

## Decision

`RuntimeConfig.description` is a `DescriptionTemplate`, not a `String`: `Settings::runtime_config` parses it, so the only paths that build a runtime config (`PUT /api/settings` with a 400 on error, and boot with `eprintln` + exit) are the only validation sites. Rendering is one exhaustive match over two private variable enums, so adding a variable is a compile error until it is wired to a context field. The Settings page lists the variable names as prose; a test pins that text against the tables.

## Consequences

A `validate()` beside a `render()` would be two copies of the grammar plus unreachable error arms. Migration 9 escapes `$` in stored descriptions because boot exits before the HTTP server starts: an unescaped pre-feature `${` would otherwise lock the user out of the UI that fixes it.
