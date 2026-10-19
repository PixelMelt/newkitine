# 32. The runtime config is parsed and validated at the settings boundary

Status: Accepted (library half moved to libsoulseek ADR 0019; refined by ADR 0046)
Nicotine+: No counterpart

## Context

The library takes a parsed `DescriptionTemplate` (libsoulseek ADR 0019) and owns the rules a `RuntimeConfig` must satisfy (libsoulseek ADR 0029), but leaves running them to the consumer.

## Decision

`Settings::runtime_config` parses the description and runs `RuntimeConfig::validate` after its own bounds, and `Settings::local_shares` runs the share rules (libsoulseek ADR 0032; ADR 0046), so the paths that build a runtime config are the only validation sites: `PUT /api/settings` with a 400 on error, boot with `eprintln` + exit, and the Gluetun forwarded-port update, which warns and retries on the next poll. The Settings page lists the description variables as prose; a test pins that text against `description_variables`.

## Consequences

Boot exits before the HTTP server starts, so a stored description that fails to parse locks the user out of the UI that fixes it; `PUT /api/settings` is what keeps one from being stored.
