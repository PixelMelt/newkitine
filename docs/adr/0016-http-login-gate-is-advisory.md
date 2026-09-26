# 16. The HTTP login gate is advisory; enforcement is actor/network-side

Status: Accepted
Nicotine+: No counterpart

## Context

`require_login` reads the projection, which lags the actor by the event queue. A TOCTOU window is inherent in any cross-task check, including an actor-owned one racing a server disconnect.

## Decision

The gate exists for UX honesty (no 202 for requests that will certainly be dropped). The network layer dropping sends while disconnected is the enforcement; durable intents (buddies, interests, wishlist) replay on login regardless.

## Consequences

Actor-owned NotLoggedIn command results were considered and rejected as ack-plumbing that still cannot close the race against the server.
