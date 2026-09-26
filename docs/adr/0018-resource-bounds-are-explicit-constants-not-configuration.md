# 18. Resource bounds are explicit constants, not configuration

Status: Accepted
Nicotine+: No counterpart

## Context

Every bound is a safety bound; a knob would only exist to be set wrong.

## Decision

Peer catalogs decompress to at most 256 MiB (a multi-million-file share is about 200 MB); payloads of 1 MiB or more parse on a blocking worker. Folder downloads cap at 1000 files per request, well under the 4096 transfer-persistence queue whose overflow is deliberately fatal. The projection retains 25 searches and 8 browse trees, evicting the oldest with events so the UI stays consistent; per-search results cap at `max_search_responses`, which stays a setting for Nicotine+ parity but is validated to 1–2000 at every input boundary. Caller-provided HTTP limits (chat history, peer stats, browse folders) have hard maxima and reject with 400 rather than silently clamping.

## Consequences

None of the retention bounds are settings.
