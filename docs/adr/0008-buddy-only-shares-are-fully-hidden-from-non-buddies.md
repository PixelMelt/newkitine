# 8. Buddy-only shares are fully hidden from non-buddies

Status: Accepted
Nicotine+: Diverges: Nicotine+ sends a locked private section in browses and locked search results

## Context

Nicotine+ reveals that buddy-only content exists to peers who cannot access it.

## Decision

Buddy-only folders are omitted entirely from browse responses, folder-contents responses and search results for non-buddies.

## Consequences

Privacy over parity. A non-buddy cannot learn what is shared with buddies.
