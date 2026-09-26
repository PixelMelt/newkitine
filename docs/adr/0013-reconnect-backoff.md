# 13. Reconnect backoff: 5 s base + 0–10 s jitter, doubling to a 300 s cap

Status: Accepted
Nicotine+: Matches in shape: Nicotine+ picks a random 5–15 s, then doubles to 300 s

## Context

Clients must spread their reconnects after a server outage.

## Decision

First delay is 5 s plus 0–10 s of jitter; each further delay doubles up to 300 s. Manual disconnect and login rejection do not auto-retry. A listen-port bind failure schedules the same backoff.

## Consequences

Jitter spreads clients after an outage without a shared clock.
