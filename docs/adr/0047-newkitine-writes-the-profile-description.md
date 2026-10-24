# 47. newkitine writes the profile description

Status: Accepted (supersedes ADR 0032 in part)
Nicotine+: Matches: Nicotine+ owns profile text and banned-user wording

## Context

The profile template needs app-owned transfer totals, while the library only needs the rendered protocol text.

## Decision

newkitine owns the template grammar, variables and banned-user text. `Profile` reads only its in-memory template and transfer totals. Boot and settings updates parse templates at the settings boundary before installing them.

## Consequences

The library supplies current client state through `ProfileRequest`; newkitine combines it with app state without I/O on the client actor.
