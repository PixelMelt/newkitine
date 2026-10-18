# 40. The projection follows the client actor's search state

Status: Accepted (library half moved to libsoulseek ADR 0026)
Nicotine+: Matches: Nicotine+ shows a wish's results on the first match and keeps a search open after its wish is removed

## Context

The client actor owns outgoing search state: it parses queries, filters responses and decides when a wish's search is shown (libsoulseek ADR 0026).

## Decision

The app never parses queries: a search the client returns no token for is rejected. The projection adds a search on `SearchStarted` and keeps one response per peer per search. Closing a search, or evicting it past the retention cap (ADR 0018), cancels it on the client, so the next matching wish run shows it again.

## Consequences

A wish occupies at most one projection slot and only once it has matches. Removing a wish whose results are shown leaves the search open until it is closed, as in Nicotine+.
