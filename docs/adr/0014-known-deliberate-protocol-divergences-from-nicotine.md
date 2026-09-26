# 14. Known-deliberate protocol divergences from Nicotine+

Status: Accepted
Nicotine+: Diverges (listed below)

## Context

A port has to choose where to stop matching the original.

## Decision

- Search answers cap at 300 results, matching current Nicotine+ (`searches/maxresults` = 300); `inqueue` in search responses reports the total queue size, not the per-requester privileged-aware figure Nicotine+ computes.
- The server message size cap is 16 MiB (Nicotine+ allows 448 MiB); no current server message approaches it, and a smaller cap bounds a hostile server.
- No UnwatchUser after transfers complete; no SetStatus away support (no UI for it yet).
- Searches arriving under our own username are never answered. Nicotine+ answers them only when we deliberately searched our own username.

## Consequences

Each item is small and documented here so audits stop re-discovering them.
