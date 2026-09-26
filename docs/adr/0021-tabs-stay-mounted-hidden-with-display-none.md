# 21. Tabs stay mounted, hidden with display:none

Status: Accepted
Nicotine+: No counterpart

## Context

Unmounting inactive tabs would drop per-tab UI state (search input, scroll position, the browse being viewed) or force lifting all of it into stores.

## Decision

All tabs stay mounted and are hidden with `display: none`.

## Consequences

Hidden tabs do react to store updates, but the projection retention caps bound that work. State preservation wins until profiling says otherwise.
