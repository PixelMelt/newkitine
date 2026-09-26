# 17. Missing default config loads defaults; missing explicit config panics

Status: Accepted
Nicotine+: No counterpart

## Context

Every bootstrap value has an environment override and a sane default.

## Decision

`newkitine.toml` absent from the working directory is a supported fresh-start state. A path explicitly set via `NEWKITINE_CONFIG` that does not exist panics at startup.

## Consequences

A typo in an explicit path fails loudly instead of silently running on defaults.
