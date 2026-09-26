# 37. The client actor owns outgoing search state

Status: Accepted
Nicotine+: Matches: Nicotine+ core keeps a `SearchRequest` per token with the sanitized term and included/excluded words, reuses one token per wish, and its search tab opens a wish's results on the first matching response

## Context

The actor sent the raw query, forgot it, and forwarded every response. Each wishlist run minted a token and a `SearchStarted`, so the projection grew one search per run and its 25-search cap evicted the user's own searches. Peers that answer loosely filled the results with files that did not contain the query's words.

## Decision

`client::search::SearchQuery` ports `_sanitize_search_term`: it produces the transmitted term and the included/excluded words. The actor keeps one `ActiveSearch` (query, parsed filter, shown flag) per allowed token, inserted when a search starts or a wish runs and removed by `CancelSearch`. Responses are filtered against it before they become `ClientEvent::SearchResults`; a response left empty is dropped. Each wish holds one token for its lifetime. A wish run allows that token and sends `WishlistSearch`, but emits `SearchStarted` only when a filtered response first arrives while the wish is not shown; the app removing that search (close or eviction) cancels it, and the next matching run shows it again. The projection keeps one response per peer per search.

## Consequences

A wish occupies at most one projection slot and only once it has matches. The app never parses queries. Removing a wish whose results are shown leaves the search open until it is closed, as in Nicotine+.
