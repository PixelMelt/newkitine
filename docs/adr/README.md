# Architecture decision records

These calls were made deliberately, with the reasoning recorded. Audits keep re-flagging them from fresh context. Do not re-open an accepted one without new evidence: "a different reviewer would have chosen differently" is not new evidence. A superseded record stays in place and says why the earlier call was wrong, so the same mistake is not rediscovered.

Each record has a `Nicotine+` line stating whether the decision matches the original client, diverges from it, or has no counterpart. The divergences are the port's deliberate choices.

To add one: next number, one decision per file, sections Context / Decision / Consequences, and mark anything it supersedes in both records.

The Soulseek library moved to [libsoulseek](https://github.com/PixelMelt/libsoulseek) with its records (ADR 0045). A moved record keeps its row here, pointing at its new number; a record that mixed both kept its app half here.

| ADR | Title | Status | Nicotine+ |
|---|---|---|---|
| 0001 | ConnControl stays one enum | Moved to [libsoulseek ADR 0001](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0001-conncontrol-stays-one-enum.md) | No counterpart |
| [0002](0002-app-list-mutation-is-one-lock-not-per-domain-locks.md) | App.list_mutation is one lock, not per-domain locks | Accepted | No counterpart |
| [0003](0003-mutation-order-is-db-first-then-client-actor-then-projection.md) | Mutation order is DB first, then client actor, then projection | Accepted | No counterpart |
| 0004 | TransferPhase::from_seed maps a missing failure reason to an empty reason | Moved to [libsoulseek ADR 0002](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0002-from-seed-null-failure-reason.md) | No counterpart |
| [0005](0005-disconnect-is-one-projection-transition.md) | Disconnect is one atomic projection transition owned by session | Accepted | No counterpart |
| [0006](0006-app-feature-files-split-only-at-ownership-boundaries.md) | App feature files split only at ownership boundaries | Accepted | No counterpart |
| 0007 | Share config change invalidates the catalog immediately | Moved to [libsoulseek ADR 0003](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0003-share-config-change-invalidates-the-catalog-immediately.md) (superseded) | Diverged: Nicotine+ keeps serving the old shares during a rescan |
| 0008 | Buddy-only shares are fully hidden from non-buddies | Moved to [libsoulseek ADR 0004](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0004-buddy-only-shares-are-fully-hidden-from-non-buddies.md) | Diverges: Nicotine+ sends a locked private section in browses and locked search results |
| 0009 | NetworkHandle::send is sync try_send + panic; Client methods are async awaited | Moved to [libsoulseek ADR 0005](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0005-network-send-sync-client-methods-async.md) | No counterpart |
| [0010](0010-the-app-event-loop-is-one-ordered-lane.md) | The app event loop is one ordered lane | Accepted | No counterpart |
| [0011](0011-owner-internal-sibling-imports-do-not-route-through-barrels.md) | Owner-internal sibling imports do not route through barrels | Accepted (library half moved to libsoulseek ADR 0006) | No counterpart |
| 0012 | Event emit() panics on Full, ignores Closed | Moved to [libsoulseek ADR 0007](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0007-event-emit-panics-on-full-ignores-closed.md) | No counterpart |
| 0013 | Reconnect backoff: 5 s base + 0–10 s jitter, doubling to a 300 s cap | Moved to [libsoulseek ADR 0008](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0008-reconnect-backoff.md) | Matches in shape: Nicotine+ picks a random 5–15 s, then doubles to 300 s |
| 0014 | Known-deliberate protocol divergences from Nicotine+ | Moved to [libsoulseek ADR 0009](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0009-known-deliberate-protocol-divergences-from-nicotine.md) | Diverges (listed below) |
| [0015](0015-transfers-are-actor-authoritative.md) | MySQL is a projection of the client actor's transfers | Accepted (library half moved to libsoulseek ADR 0010) | No counterpart |
| [0016](0016-http-login-gate-is-advisory.md) | The HTTP login gate is advisory; enforcement is actor/network-side | Accepted | No counterpart |
| [0017](0017-missing-config-handling.md) | Missing default config loads defaults; missing explicit config panics | Accepted | No counterpart |
| [0018](0018-resource-bounds-are-explicit-constants-not-configuration.md) | Resource bounds are explicit constants, not configuration | Accepted (library half moved to libsoulseek ADR 0011) | No counterpart |
| [0019](0019-terminal-transfers-stay-in-projection.md) | Terminal transfers stay in the projection until the user clears them | Accepted | Matches Nicotine+ |
| [0020](0020-chat-history-has-two-owners-the-database-and-the-browser.md) | Chat history has two owners: the database and the browser | Accepted | No counterpart |
| [0021](0021-tabs-stay-mounted-hidden-with-display-none.md) | Tabs stay mounted, hidden with display:none | Accepted | No counterpart |
| [0022](0022-projection-payload-serialization.md) | Bounded projection payloads serialize under the lock; unbounded ones are Arc | Accepted | No counterpart |
| 0023 | Download placement runs on a blocking worker with a Placing phase | Moved to [libsoulseek ADR 0012](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0012-download-placement-on-blocking-worker.md) | No counterpart |
| [0024](0024-browser-contract-covers-websocket-only.md) | The typed browser contract covers the WebSocket stream, not REST responses | Accepted | No counterpart |
| 0025 | The share catalog cache is fail-open by design | Moved to [libsoulseek ADR 0013](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0013-the-share-catalog-cache-is-fail-open-by-design.md) | Matches: Nicotine+ rescans when its share databases fail to load |
| [0026](0026-pushover-delivery-best-effort.md) | Pushover delivery is best-effort: bounded queue, drop-on-full, warn-on-error | Accepted | No counterpart |
| 0027 | Scan lifecycle vocabulary | Moved to [libsoulseek ADR 0014](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0014-scan-lifecycle-vocabulary.md) | No counterpart |
| 0028 | Browse responses are encoded once per index, not per request | Moved to [libsoulseek ADR 0015](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0015-browse-responses-are-encoded-once-per-index-not-per-request.md) | Matches: Nicotine+ builds compressed share lists at scan time and drops repeat requests within 0.4 s |
| 0029 | Search answering stays inline on the client actor | Moved to [libsoulseek ADR 0016](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0016-search-answering-stays-inline-on-the-client-actor.md) | No counterpart |
| 0030 | Deferred-request overflow answers "Too many files" | Moved to [libsoulseek ADR 0017](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0017-deferred-request-overflow-answers-too-many-files.md) (superseded) | No counterpart |
| 0031 | The attribute cache is skipped when the scan changed nothing | Moved to [libsoulseek ADR 0018](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0018-the-attribute-cache-is-skipped-when-the-scan-changed-nothing.md) (superseded) | No counterpart |
| [0032](0032-description-template-parsed-at-settings-boundary.md) | The runtime config is parsed and validated at the settings boundary | Accepted (library half moved to libsoulseek ADR 0019; refined by ADR 0046) | No counterpart |
| [0033](0033-walking-the-shared-folders-on-startup-is-opt-in.md) | Walking the shared folders on startup is opt-in | Accepted (rewritten with libsoulseek ADR 0020) | Diverges: Nicotine+ defaults `rescanonstartup` to on |
| 0034 | The share catalog is persisted and served at boot | Moved to [libsoulseek ADR 0020](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0020-the-share-catalog-is-persisted-and-served-at-boot.md) | Matches: Nicotine+ persists its share databases, serves them at login, and keeps serving the old list during a rescan |
| 0035 | Installing an index re-validates active uploads | Moved to [libsoulseek ADR 0021](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0021-installing-an-index-re-validates-active-uploads.md) | Stricter than Nicotine+, which does not re-check uploads against a new share list |
| [0036](0036-repeat-downloads-are-capped-per-file-not-convicted-per-user.md) | Repeat downloads are capped per file, not convicted per user | Accepted (library half moved to libsoulseek ADR 0022) | No counterpart |
| 0037 | Speed limits are one shared budget per direction | Moved to [libsoulseek ADR 0023](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0023-speed-limits-are-one-shared-budget-per-direction.md) | Matches the default total limit; Nicotine+ divides it by the active transfer count instead of sharing a budget |
| 0038 | Peer connection close is immediate and surfaces unsent messages | Moved to [libsoulseek ADR 0024](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0024-peer-connection-close-is-immediate-and-surfaces-unsent-messages.md) | Matches: Nicotine+ clears closing buffers, re-routes later sends and reports unprocessed messages |
| 0039 | The share walk follows symlinks and skips colliding paths | Moved to [libsoulseek ADR 0025](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0025-the-share-walk-follows-symlinks-and-skips-colliding-paths.md) | Matches: Nicotine+ follows symlinks and skips already-shared decoded paths |
| [0040](0040-the-client-actor-owns-outgoing-search-state.md) | The projection follows the client actor's search state | Accepted (library half moved to libsoulseek ADR 0026) | Matches: Nicotine+ shows a wish's results on the first match and keeps a search open after its wish is removed |
| 0041 | Download recovery resumes recoverable failures only | Moved to [libsoulseek ADR 0027](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0027-download-recovery-resumes-recoverable-failures-only.md) | Diverges: Nicotine+ re-enqueues every failed download when its uploader comes online |
| [0042](0042-the-url-owns-location.md) | The URL owns location in the web UI | Accepted | No counterpart |
| [0043](0043-every-mutating-call-reports-its-outcome.md) | Every mutating call in the web UI reports its outcome through one wrapper | Accepted | No counterpart |
| 0044 | Client commands are closures run on the actor | Moved to [libsoulseek ADR 0028](https://github.com/PixelMelt/libsoulseek/blob/master/docs/adr/0028-client-commands-are-closures-run-on-the-actor.md) | No counterpart |
| [0045](0045-the-soulseek-library-lives-in-libsoulseek.md) | The Soulseek library lives in libsoulseek | Accepted | No counterpart |
| [0046](0046-settings-build-the-local-storage-and-replace-it-on-change.md) | Settings build the local storage and replace it on change | Accepted (refines ADR 0032) | No counterpart |
