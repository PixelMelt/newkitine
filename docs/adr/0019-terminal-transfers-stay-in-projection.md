# 19. Terminal transfers stay in the projection until the user clears them

Status: Accepted
Nicotine+: Matches Nicotine+

## Context

The transfer list mirrors the actor's full authoritative list, including finished, aborted and failed rows.

## Decision

Nothing trims terminal rows behind the user's back. Autoclear and clear-all are the pressure valves.

## Consequences

This data is user-generated, not peer-controlled: its size is bounded by the user's own transfer volume. Snapshot serialization cost follows the bounded-payload rule (ADR 0022).
