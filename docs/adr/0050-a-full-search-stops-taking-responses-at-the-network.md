# 50. A full search stops taking responses at the network

Status: Accepted
Nicotine+: Matches: Nicotine+ calls `remove_allowed_token` once a search shows `max_displayed_results`

## Context

The projection caps each search at `max_search_responses` (ADR 0018), but the cap applied only at the end of the chain. After a search was full, every further answer was still decompressed by libsoulseek and passed through the network actor, the client actor and the app event loop, only to be dropped. A survey's searches stayed open, so most of its event flood was answers nobody kept, and that flood is what filled the event queues (libsoulseek ADR 0038).

## Decision

When a search holds `max_search_responses` responses, `apply_results` calls `Client::stop_search_results`. libsoulseek then drops the search's further responses at the header check, before decompressing them. The search stays open and keeps the responses it has. A wishlist search re-allows its token each time it runs, and the next response stops it again, as in Nicotine+.

## Consequences

Raising `max_search_responses` does not reopen a search that was already stopped; a new search uses the new cap. A response already past the header check when the stop arrives still reaches the projection, which drops it as before.
