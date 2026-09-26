# 30. Deferred-request overflow answers "Too many files"

Status: Superseded by ADR 0034
Nicotine+: No counterpart

## Context

Upload requests arriving while no index existed were buffered, up to 4096, and replayed when the index installed.

## Decision

Beyond the buffer, deny with "Too many files", which Nicotine+ peers display as Queued and retry, rather than "File not shared.", which makes them re-request once as latin-1 and then fail. The limit was raised from 128 (exhausted on every boot) to 4096 (never reached on the reference 850k-file share).

## Consequences

The buffer and its overflow policy existed only because the index was absent for minutes at boot and after every share edit. ADR 0034 installs the cached catalog in seconds, so the buffer, the limit, the replay and the `awaiting_share_index` gate were deleted.
