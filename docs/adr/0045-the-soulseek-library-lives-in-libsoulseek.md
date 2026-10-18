# 45. The Soulseek library lives in libsoulseek

Status: Accepted
Nicotine+: No counterpart

## Context

`protocol`, `network`, `client` and `types` never imported the app; the app reached them only through the `client` and `types` barrels. Being one crate let app concerns leak into the library unnoticed: app-only types in `types`, serde shapes chosen for the browser, and Soulseek config rules checked only in the app's settings.

## Decision

The library is the `soulseek` crate in its own repository, [libsoulseek](https://github.com/PixelMelt/libsoulseek), with its integration tests and its decision records. newkitine depends on it as a git dependency pinned by `Cargo.lock`, and is a binary crate whose modules sit at the crate root. Types only the app uses live in the app, and the library's records were renumbered there; this index maps the old numbers.

## Consequences

A library change lands in libsoulseek first and reaches newkitine through `cargo update -p soulseek`. The crate boundary now enforces what ADR 0011 asked of the barrels. Library logs carry the `soulseek` target, so the default filter names both crates.
