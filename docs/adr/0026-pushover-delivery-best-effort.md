# 26. Pushover delivery is best-effort: bounded queue, drop-on-full, warn-on-error

Status: Accepted
Nicotine+: No counterpart

## Context

Pushover is an external third-party sink; an outage there, or a DM flood, must not block the ordered event lane or kill the app.

## Decision

Notifications feed a bounded channel drained by one worker; enqueue is `try_reserve` with warn + drop, and delivery failures warn and move on. Keys are captured at enqueue time so the worker owns no `App` reference.

## Consequences

This deliberately differs from the internal event stream's panic-on-Full (ADR 0012). The event that triggered the notification is already persisted and projected before the notification is enqueued; nothing diverges when one is dropped.
