# 42. The URL owns location in the web UI

Status: Accepted
Nicotine+: No counterpart

## Context

"Where the user is" had no owner. The active tab was a plain store that reset to Search on every reload, each tab kept its selected chat, room, search, share list, profile or settings page in a component variable, and cross-tab jumps ("Browse Files", "Send Message") travelled through four one-shot mailbox stores that the destination tab consumed and cleared. Reload lost every position, the browser's Back button left the app, and nothing could be bookmarked or opened in a second window.

## Decision

`lib/ui.js` owns one `route` store, parsed from and written to `location.pathname`: `/<tab>`, `/<tab>/<item>`, and for Browse `/browse/<user>/<folder…>`. The item is the selected search token, username, room or settings page. The store also remembers the last item of every tab and is mirrored to `localStorage`, so `/` and a tab switch return to where the user was.

Tabs read their selection from the route and change it by calling `go`. `go` pushes history when the tab or item changes and replaces it for a folder change inside one share list. `remember` and `forget` adjust a tab's item without navigating. The mailbox stores are gone; `openBrowse`, `openPrivateChat` and `openUserInfo` are navigations.

A route never causes a network action before the first snapshot has arrived, and restoring a route never opens a chat or joins a room: only the explicit `open…` calls do.

## Consequences

Reload, Back and Forward, and Ctrl-click on a section all work, and the server needs no change because it already serves `index.html` for unknown paths. Tabs still stay mounted (ADR 0021); the route only decides which pane is shown and which item each pane selects. A stale item in a restored route (a closed chat, an evicted search) selects nothing rather than recreating the thing.
