# 35. Installing an index re-validates active uploads

Status: Accepted (supersedes ADR 0007)
Nicotine+: Stricter than Nicotine+, which does not re-check uploads against a new share list

## Context

ADR 0007 revoked every active upload with "Cancelled" whenever the share configuration changed, including uploads of files that remained shared.

## Decision

`Uploads::revalidate` runs on every index install: each active upload whose virtual path no longer resolves for that peer (buddy access included), or now resolves to a different real file than the one queued, is denied with "File not shared." (connection closed, UploadDenied sent, transfer failed); every other upload continues untouched. The same `deny_each` helper backs `deny_all` for restrictions.

Resolution is case-insensitive, so a path whose exact-case file was filtered out can still resolve to a sibling differing only in case; comparing the stored real path keeps the upload from serving the file that is no longer shared.

## Consequences

Adding a folder no longer cancels anyone. Removing a folder, or a peer losing buddy access to one, stops the upload at the next install, which for share edits is within seconds (ADR 0034). Nicotine+ peers receiving "File not shared." retry once with latin-1 and then fail; that is the truthful end state for a file we no longer share.
