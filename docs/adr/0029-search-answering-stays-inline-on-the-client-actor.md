# 29. Search answering stays inline on the client actor

Status: Accepted
Nicotine+: No counterpart

## Context

Moving matching to a blocking lane would require re-validating ban, buddy and catalog state at completion and reordering responses.

## Decision

Matching walks the seed word's sorted postings (smallest expanded posting first, folder ranges merged lazily) and probes the other words by binary search, stopping at `max_results`. Partial (`*suffix`) words are the exception: their union is materialized from a reversed-word sorted list, bounded by the size of the union rather than the whole vocabulary.

## Consequences

A common-word query costs about 100 µs on an 850k-file catalog, so a blocking lane would buy nothing.
