# 18. Resource bounds are explicit constants, not configuration

Status: Accepted
Nicotine+: No counterpart

## Context

Every bound is a safety bound; a knob would only exist to be set wrong.

## Decision

Peer catalogs decompress to at most 256 MiB (a multi-million-file share is about 200 MB); payloads of 1 MiB or more, and every compressed peer response, parse on a blocking worker. Shared-list and user-info response frames may reach 448 MiB, as in Nicotine+, and are only read from peers we asked. Search and folder-contents responses inflate at most a 64 KiB header before the token or folder is checked against our outstanding requests, as Nicotine+ inflates only the header; the rest inflates to at most 128 MiB. A bound on an optional part of a peer response trims that part instead of dropping the message: each search result list keeps its first 5000 files, and a user picture over 8 MiB is omitted while the rest of the user info is kept. Nicotine+ has neither of those two caps. Folder downloads cap at 1000 files per request, well under the 4096 transfer-persistence queue whose overflow is deliberately fatal. The projection retains 25 searches and 8 browse trees, evicting the oldest with events so the UI stays consistent; per-search results cap at `max_search_responses`, which stays a setting for Nicotine+ parity but is validated to 1–2000 at every input boundary. Caller-provided HTTP limits (chat history, peer stats, browse folders) have hard maxima and reject with 400 rather than silently clamping.

## Consequences

None of the retention bounds are settings.
