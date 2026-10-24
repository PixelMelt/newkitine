# 32. The runtime config is parsed and validated at the settings boundary

Status: Accepted (library half moved to libsoulseek ADR 0019, since superseded by libsoulseek ADR 0033; description half superseded by ADR 0047; refined by ADR 0046)
Nicotine+: No counterpart

## Context

The app owns the description template (ADR 0047), while the library owns the rules a `RuntimeConfig` must satisfy (libsoulseek ADR 0029) and leaves running them to the consumer.

## Decision

`Settings::runtime_config` runs `RuntimeConfig::validate` after its own bounds, and `Settings::local_shares` runs the share rules (libsoulseek ADR 0032; ADR 0046). `Settings::description_template` parses profile text for `PUT /api/settings` with a 400 on error and boot with `eprintln` + exit; the Gluetun forwarded-port path only rebuilds runtime configuration. The Settings page lists the description variables as prose; a test pins that text against the app's `description_variables`.

## Consequences

Boot exits before the HTTP server starts, so a stored description that fails to parse locks the user out of the UI that fixes it; `PUT /api/settings` is what keeps one from being stored.
