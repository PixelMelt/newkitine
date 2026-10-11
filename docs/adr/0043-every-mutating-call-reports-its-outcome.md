# 43. Every mutating call in the web UI reports its outcome through one wrapper

Status: Accepted
Nicotine+: No counterpart

## Context

Most `post`/`put`/`del` call sites dropped their promise. A rejected request (not logged in, server unreachable, invalid input) surfaced only in the browser console, several views were left in a waiting state that could never end, and text fields were cleared before the request that carried their text had succeeded. Successes were just as silent. The one feedback channel, `notice()`, showed its last message in red forever.

The browser-to-server socket also had no state of its own, so before the first snapshot and whenever the server was unreachable the status bar presented default values ("Disconnected", port 0, zero shares) as facts.

## Decision

`lib/act.js` exports `act(request, { failed, done, action, on, where, then })`. Every mutating call goes through it. A failure becomes one sentence, "`<failed>`: `<cause>`.", where the cause is derived from the status code; a success may report `done` with an optional follow-up action such as Undo. It resolves to whether the request succeeded, and inputs are cleared only on success. A caller with a better place for the message than the status bar passes `where`.

`report` in `lib/state/session.js` replaces `notice()`: one slot in the status bar that shows the latest entry and expires. Only errors are red. A window-level `unhandledrejection` handler is the backstop.

`link` in `lib/state/socket.js` is the state of the browser-to-server socket (`connecting`, `live`, `lost`, `broken`), separate from `status`, which is the server-to-Soulseek state. The status bar renders from `link` first. A login failure is state (`loginFailure`), not a message, so later messages cannot overwrite the reason the client is offline.

Destructive actions that the frontend can reverse are done at once and offer Undo in the slot instead of asking first. Only clearing a list with running transfers keeps a guard, and it is an in-place second button, not a dialog. The UI has no `alert`, `confirm` or `prompt`.

## Consequences

No failure is invisible, and no view claims something it does not know. Bulk actions report once, not once per row. The cost is that a new mutating call written as a bare `post(...)` regresses silently until it fails; the backstop then reports it as an unexpected error, which is the cue to wrap it.
