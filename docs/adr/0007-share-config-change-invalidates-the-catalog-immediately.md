# 7. Share config change invalidates the catalog immediately

Status: Superseded by ADR 0034 and ADR 0035
Nicotine+: Diverged: Nicotine+ keeps serving the old shares during a rescan

## Context

When the share configuration changed, the old grants were considered void.

## Decision

Drop the index, revoke every active upload with UploadDenied "Cancelled" (not "File not shared.", which triggers the legacy latin-1 retry in Nicotine+ peers), advertise 0/0, and defer inbound queue requests until the new index installs. A plain rescan with unchanged config kept serving the old index.

## Consequences

This was the wrong shape. Revoking every upload on any share edit cancelled peers' queued downloads for files that were still shared, and the index-less window was the reason the request deferral subsystem (ADR 0030) existed at all. ADR 0034 installs the cached catalog restricted to the new config within seconds while the old index keeps serving, and ADR 0035 denies only the uploads the new index no longer resolves.
