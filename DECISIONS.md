# Settled design decisions

These calls were made deliberately, with the reasoning recorded here. Audits keep re-flagging them from fresh context. Do not re-open them without new evidence — "a different reviewer would have chosen differently" is not new evidence.

## ConnControl stays one enum

One control channel follows a connection across its phase transitions (pre-init → message loop or file loop). Splitting into per-phase control types forces a channel handoff at each transition, which races against controls already queued on the old channel. The `unreachable!` arms in the conn loops are fail-loud guards on actor routing invariants, which is exactly the failure mode we want. Two independent audit sessions examined the split and both withdrew it.

## App.list_mutation is one lock, not per-domain locks

Durable list mutations (buddies, bans, ignores, IP bans, wishlist, interests, chat partners, rooms) serialize behind a single mutex. These are rare, human-driven operations; contention is negligible and one lock is impossible to deadlock against itself. Per-domain locks would add surface for lock-ordering bugs while buying nothing measurable. The lock's job is ordering the DB → client-actor → projection write sequence against concurrent opposite mutations of the same key, and one lock does that job completely.

## Mutation order is DB first, then client actor, then projection

The database is the durable source of truth; a crash after the DB write converges on restart because boot reloads from the DB. Reversing the order (actor first) can acknowledge state that was never persisted.

## TransferPhase::from_seed uses unwrap_or_default on failure_reason

Failed transfer rows may legitimately carry NULL failure_reason (rows that predate migration 2, which extracted reasons from the old status strings). Mapping NULL to an empty reason is handling real data, not masking a failed code path. Making it an error would crash boot on valid legacy databases.

## Disconnect is one atomic projection transition owned by session

`session::disconnected` holds one projection write lock and delegates to `chat::server_disconnected` and `users::server_disconnected`, which take the writer. Rooms cleared with RoomLeft events, buddy statuses reset to "unknown", peer count zeroed, available rooms cleared, then the Status event. Moving this into a separate "event coordinator" adds a layer that owns nothing else.

## App feature files split only at ownership boundaries

`chat.rs`, `search.rs`, `interests.rs`, `stats.rs` each hold their domain state, SQL, event appliers, and HTTP handlers. `transfers/` and `users/` were split into folders because their persistence and HTTP surfaces grew genuine sub-owners. The others get folders when — and only when — an ownership boundary appears, not because a file mixes concerns at small scale.

## Share config change invalidates the catalog immediately

Deliberate divergence from Nicotine+, which keeps serving the old shares during a rescan. When the share *configuration* changes, the old grants are void: the index is dropped, active uploads are revoked with UploadDenied "Cancelled" (not "File not shared.", which triggers Nicotine+ peers' legacy latin-1 retry), 0/0 is advertised, and requests defer until the new index installs. A plain rescan with unchanged config keeps serving the old index, matching Nicotine+.

## Buddy-only shares are fully hidden from non-buddies

Nicotine+ sends non-accessible shares as a locked "private" browse section and as locked search results. We omit them entirely. Privacy over parity.

## NetworkHandle::send is sync try_send + panic; Client methods are async awaited

The client actor and network actor form a command cycle; awaiting inside it deadlocks. Actor-to-actor sends are try_send with panic on Full (queue overflow is a fatal pacing bug, not a recoverable condition). The app→client boundary has no cycle, so Client methods await with real backpressure.

## The app event loop is one ordered lane

Client events apply strictly in order on a single task, including their database writes. Per-domain worker lanes were considered and rejected: room lifecycle, chat, session disconnect, and user state have cross-domain ordering dependencies (a disconnect atomically clears rooms that queued room events would then race), and the projection's correctness leans on the total order the single lane provides. Transfers got a dedicated worker precisely because their ordering is self-contained behind actor-authoritative snapshots. A database slow enough to back up the event queue is the "app cannot keep up" condition the fatal queue policy already covers.

## Owner-internal sibling imports do not route through barrels

Barrels are the contract for consumers *outside* an owning module (app importing client, client importing network). Within an owner's subtree — protocol files using wire.rs, transfers using client::users, network actors using network::conn — direct imports are the norm; routing internals through the owner's own barrel adds indirection with no boundary being defended.

## Event emit() panics on Full, ignores Closed

Consumer death is detected by supervision (events::run exiting kills the process); a closed channel during shutdown is not an error. A full event queue means the app cannot keep up with the client actor and losing events would silently diverge state — panic is the loud option.

## Reconnect backoff: 5s base + 0–10s jitter, doubling to 300s cap

Matches Nicotine+'s shape (theirs is random 5–15s then doubling). Jitter exists to spread clients after a server outage. Manual disconnect and login rejection do not auto-retry. Listen-port bind failure schedules the same backoff.

## Known-deliberate protocol divergences from Nicotine+

- Search answers cap at 300 results, matching current nicotine (searches/maxresults = 300); inqueue reports total queue size, not the per-requester privileged-aware figure.
- Server message size cap is 16 MiB (nicotine 448 MiB); no current server message approaches it, and a smaller cap bounds a hostile server.
- No UnwatchUser after transfers complete; no SetStatus away support (no UI for it yet).
- Searches from our own username are never answered (nicotine allows deliberate self-search).

## Transfers are actor-authoritative; MySQL is a projection of them

Durable-list ordering (DB first) does not apply to transfers. The client actor owns transfer state and emits authoritative TransferSnapshot transitions; the app persists them through one ordered worker, and a DB write failure is fatal (exit). The actor seeds from the DB at boot, so a crash between acknowledgement and persistence loses at most the in-window transitions — accepted and bounded. This was chosen over app-owned prepare/persist/commit across four audit rounds because transfer transitions originate on the network side (peer messages), where the actor must decide immediately; routing every transition through the app before acting would add a round-trip on the hot path and a second owner for retry/queue logic.

## The HTTP login gate is advisory; enforcement is actor/network-side

`require_login` reads the projection, which lags the actor by the event queue — a TOCTOU window is inherent in any cross-task check, including an actor-owned one racing a server disconnect. The gate exists for UX honesty (no 202 for requests that will certainly be dropped); the network layer dropping sends while disconnected is the enforcement; durable intents (buddies, interests, wishlist) replay on login regardless. Actor-owned NotLoggedIn command results were considered and rejected as ack-plumbing that still cannot close the race against the server.

## Missing default config loads defaults; missing explicit config panics

`newkitine.toml` absent from the working directory is a supported fresh-start state (every value has an env override and a sane default). A path explicitly set via NEWKITINE_CONFIG that does not exist panics at startup.

## Resource bounds are explicit constants, not configuration

Peer catalogs decompress to at most 256 MiB (a multi-million-file share is ~200 MB); payloads of 1 MiB or more parse on a blocking worker. Folder downloads cap at 1000 files per request, well under the 4096 transfer-persistence queue whose overflow is deliberately fatal. The projection retains 25 searches and 8 browse trees, evicting oldest with events so the UI stays consistent; per-search results cap at max_search_responses, which stays a setting for nicotine parity but is validated to 1-2000 at every input boundary. Caller-provided HTTP limits (chat history, peer stats, browse folders) have hard maxima and reject with 400 rather than silently clamping. None of the retention bounds are settings: they are safety bounds, and a knob would only exist to be set wrong.

## Terminal transfers stay in the projection until the user clears them

The transfer list mirrors the actor's full authoritative list, including finished/aborted/failed rows, exactly like Nicotine+. Trimming terminal rows behind the user's back would contradict the clear buttons and the autoclear settings that exist precisely to let the user choose. This data is user-generated, not peer-controlled: its size is bounded by the user's own transfer volume, and both autoclear and clear-all already exist as the pressure valves. Snapshot serialization cost follows the same bounded-payload rule as the rest of the projection.

## Chat history has two owners: the database and the browser

The projection holds no chat messages — not private (migrated first) and not room messages. The database is durable history served over REST; the browser merges that history with live events, reconciling on database message identity. Room views in the projection carry only membership.

## Tabs stay mounted, hidden with display:none

Unmounting inactive tabs would drop per-tab UI state (search input, scroll, the browse being viewed) or force lifting all of it into stores. Hidden tabs do react to store updates, but the projection retention caps bound that work. State preservation wins until profiling says otherwise.

## Bounded projection payloads serialize under the lock; unbounded ones are Arc

Snapshot and search serialization happen under the projection read lock — with the retention caps above they are bounded and the clone needed to serialize outside would cost the same as serializing. Browse trees are the unbounded payload, so BrowseView holds Arc'd folder lists: readers clone the Arc under the lock and do all tree-walking and serialization after release. Chat history has exactly two owners — the database and the browser event stream; the projection deliberately holds none of it.

## Download placement runs on a blocking worker with a Placing phase

The client actor never does unbounded filesystem work: on completion a transfer enters TransferPhase::Placing (externally still "transferring"), rename/copy runs via spawn_blocking, and the result returns through a channel the actor selects on. Copy-fallback happens only on cross-device rename failure; other errors fail the transfer loudly. Placing counts as active (blocks duplicate enqueue) but is excluded from disconnect reset, so placement survives a server drop.

## The typed browser contract covers the WebSocket stream, not REST responses

WebSocket events mutate long-lived replicated stores, where a silently drifted shape corrupts state that outlives the message — so that surface is typed, validated at the socket boundary, and pinned by the node-backed contract test. REST responses are consumed once, at the call site that requested them, and a shape mismatch fails visibly in that one component on the next interaction. Typing every ad-hoc REST payload would add a parallel DTO layer whose drift risk it exists to prevent. The contract module's bidirectional imports with feature modules are inherent to an app-owned internally-tagged event enum, not a layering defect.

## The scan cache is fail-open by design

The attribute cache is derived data: every entry can be rebuilt from the files themselves. A missing, unreadable, or corrupt cache warns and rescans from scratch; a failed save warns and continues serving the fresh index. This is a deliberate exception to the no-fallbacks rule — failing the scan over a cache problem would turn a performance sidecar into a correctness dependency. Share data itself stays fail-loud: scan errors abort the scan and surface as ShareScanFailed.

## Scan lifecycle vocabulary

`Sharing.running` is the single owner of "a scan task is in flight" (set by spawn_scan, cleared by handle_scan_complete). `awaiting_share_index()` = no index && (running || rescan_pending) is the only deferral gate, used for inbound queue requests and outgoing QueueUpload deferral. `send_queue_request` is the only emitter of outgoing QueueUpload in Downloads; `flush_queued_requests` is its drain side. Generation bumps invalidate in-flight results; rescans coalesce rather than run concurrently.
