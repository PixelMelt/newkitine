# 18. Resource bounds are explicit constants, not configuration

Status: Accepted (library half moved to libsoulseek ADR 0011)
Nicotine+: No counterpart

## Context

Every bound is a safety bound; a knob would only exist to be set wrong. The library's own bounds on peer data and transfers are libsoulseek ADR 0011.

## Decision

The projection retains 25 searches and 8 browse trees, evicting the oldest with events so the UI stays consistent; per-search results cap at `max_search_responses`, which stays a setting for Nicotine+ parity but is validated to 1–2000 at every input boundary. A folder download larger than the library's `FOLDER_DOWNLOAD_FILE_LIMIT` is rejected with 400 before any file is enqueued. Caller-provided HTTP limits (chat history, peer stats, browse folders) have hard maxima and reject with 400 rather than silently clamping.

## Consequences

None of the retention bounds are settings.
