# Architecture decision records

These calls were made deliberately, with the reasoning recorded. Audits keep re-flagging them from fresh context. Do not re-open an accepted one without new evidence: "a different reviewer would have chosen differently" is not new evidence. A superseded record stays in place and says why the earlier call was wrong, so the same mistake is not rediscovered.

Each record has a `Nicotine+` line stating whether the decision matches the original client, diverges from it, or has no counterpart. The divergences are the port's deliberate choices.

To add one: next number, one decision per file, sections Context / Decision / Consequences, and mark anything it supersedes in both records.

| ADR | Title | Status | Nicotine+ |
|---|---|---|---|
| [0001](0001-conncontrol-stays-one-enum.md) | ConnControl stays one enum | Accepted | No counterpart |
| [0002](0002-app-list-mutation-is-one-lock-not-per-domain-locks.md) | App.list_mutation is one lock, not per-domain locks | Accepted | No counterpart |
| [0003](0003-mutation-order-is-db-first-then-client-actor-then-projection.md) | Mutation order is DB first, then client actor, then projection | Accepted | No counterpart |
| [0004](0004-from-seed-null-failure-reason.md) | TransferPhase::from_seed maps a NULL failure_reason to an empty reason | Accepted | No counterpart |
| [0005](0005-disconnect-is-one-projection-transition.md) | Disconnect is one atomic projection transition owned by session | Accepted | No counterpart |
| [0006](0006-app-feature-files-split-only-at-ownership-boundaries.md) | App feature files split only at ownership boundaries | Accepted | No counterpart |
| [0007](0007-share-config-change-invalidates-the-catalog-immediately.md) | Share config change invalidates the catalog immediately | Superseded by ADR 0034 and ADR 0035 | Diverged: Nicotine+ keeps serving the old shares during a rescan |
| [0008](0008-buddy-only-shares-are-fully-hidden-from-non-buddies.md) | Buddy-only shares are fully hidden from non-buddies | Accepted | Diverges: Nicotine+ sends a locked private section in browses and locked search results |
| [0009](0009-network-send-sync-client-methods-async.md) | NetworkHandle::send is sync try_send + panic; Client methods are async awaited | Accepted | No counterpart |
| [0010](0010-the-app-event-loop-is-one-ordered-lane.md) | The app event loop is one ordered lane | Accepted | No counterpart |
| [0011](0011-owner-internal-sibling-imports-do-not-route-through-barrels.md) | Owner-internal sibling imports do not route through barrels | Accepted | No counterpart |
| [0012](0012-event-emit-panics-on-full-ignores-closed.md) | Event emit() panics on Full, ignores Closed | Accepted | No counterpart |
| [0013](0013-reconnect-backoff.md) | Reconnect backoff: 5 s base + 0–10 s jitter, doubling to a 300 s cap | Accepted | Matches in shape: Nicotine+ picks a random 5–15 s, then doubles to 300 s |
| [0014](0014-known-deliberate-protocol-divergences-from-nicotine.md) | Known-deliberate protocol divergences from Nicotine+ | Accepted | Diverges (listed below) |
| [0015](0015-transfers-are-actor-authoritative.md) | Transfers are actor-authoritative; MySQL is a projection of them | Accepted | No counterpart |
| [0016](0016-http-login-gate-is-advisory.md) | The HTTP login gate is advisory; enforcement is actor/network-side | Accepted | No counterpart |
| [0017](0017-missing-config-handling.md) | Missing default config loads defaults; missing explicit config panics | Accepted | No counterpart |
| [0018](0018-resource-bounds-are-explicit-constants-not-configuration.md) | Resource bounds are explicit constants, not configuration | Accepted | No counterpart |
| [0019](0019-terminal-transfers-stay-in-projection.md) | Terminal transfers stay in the projection until the user clears them | Accepted | Matches Nicotine+ |
| [0020](0020-chat-history-has-two-owners-the-database-and-the-browser.md) | Chat history has two owners: the database and the browser | Accepted | No counterpart |
| [0021](0021-tabs-stay-mounted-hidden-with-display-none.md) | Tabs stay mounted, hidden with display:none | Accepted | No counterpart |
| [0022](0022-projection-payload-serialization.md) | Bounded projection payloads serialize under the lock; unbounded ones are Arc | Accepted | No counterpart |
| [0023](0023-download-placement-on-blocking-worker.md) | Download placement runs on a blocking worker with a Placing phase | Accepted | No counterpart |
| [0024](0024-browser-contract-covers-websocket-only.md) | The typed browser contract covers the WebSocket stream, not REST responses | Accepted | No counterpart |
| [0025](0025-the-share-catalog-cache-is-fail-open-by-design.md) | The share catalog cache is fail-open by design | Accepted | Matches: Nicotine+ rescans when its share databases fail to load |
| [0026](0026-pushover-delivery-best-effort.md) | Pushover delivery is best-effort: bounded queue, drop-on-full, warn-on-error | Accepted | No counterpart |
| [0027](0027-scan-lifecycle-vocabulary.md) | Scan lifecycle vocabulary | Accepted | No counterpart |
| [0028](0028-browse-responses-are-encoded-once-per-index-not-per-request.md) | Browse responses are encoded once per index, not per request | Accepted | Matches: Nicotine+ builds compressed share lists at scan time and drops repeat requests within 0.4 s |
| [0029](0029-search-answering-stays-inline-on-the-client-actor.md) | Search answering stays inline on the client actor | Accepted | No counterpart |
| [0030](0030-deferred-request-overflow-answers-too-many-files.md) | Deferred-request overflow answers "Too many files" | Superseded by ADR 0034 | No counterpart |
| [0031](0031-the-attribute-cache-is-skipped-when-the-scan-changed-nothing.md) | The attribute cache is skipped when the scan changed nothing | Superseded by ADR 0034 | No counterpart |
| [0032](0032-description-template-parsed-at-settings-boundary.md) | The description template is parsed at the settings boundary, not validated then rendered | Accepted | No counterpart |
| [0033](0033-walking-the-shared-folders-on-startup-is-opt-in.md) | Walking the shared folders on startup is opt-in | Accepted | Diverges: Nicotine+ defaults `rescanonstartup` to on |
| [0034](0034-the-share-catalog-is-persisted-and-served-at-boot.md) | The share catalog is persisted and served at boot | Accepted | Matches: Nicotine+ persists its share databases, serves them at login, and keeps serving the old list during a rescan |
| [0035](0035-installing-an-index-re-validates-active-uploads.md) | Installing an index re-validates active uploads | Accepted | Stricter than Nicotine+, which does not re-check uploads against a new share list |
| [0036](0036-repeat-downloads-are-capped-per-file-not-convicted-per-user.md) | Repeat downloads are capped per file, not convicted per user | Accepted | No counterpart |
| [0037](0037-speed-limits-are-one-shared-budget-per-direction.md) | Speed limits are one shared budget per direction | Accepted | Matches the default total limit; Nicotine+ divides it by the active transfer count instead of sharing a budget |
| [0038](0038-peer-connection-close-is-immediate-and-surfaces-unsent-messages.md) | Peer connection close is immediate and surfaces unsent messages | Accepted | Matches: Nicotine+ clears closing buffers, re-routes later sends and reports unprocessed messages |
| [0039](0039-the-share-walk-follows-symlinks-and-skips-colliding-paths.md) | The share walk follows symlinks and skips colliding paths | Accepted | Matches: Nicotine+ follows symlinks and skips already-shared decoded paths |
| [0040](0040-the-client-actor-owns-outgoing-search-state.md) | The client actor owns outgoing search state | Accepted | Matches: Nicotine+ keeps per-token search requests, one token per wish, and shows wish results on the first match |
