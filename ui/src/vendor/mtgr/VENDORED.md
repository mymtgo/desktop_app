# Vendored .mtgr parser

Source: `mtgreplay/typescript-parser` at commit `f8918fcb8c0fcedf5b6be7ff6a998efb354a15c9` on `main`
(`pt ... -> none` clears power and toughness, section 7.5). The format version stays 0.3.0.
Files: `src/*.ts` and `LICENSE` (MIT), copied unchanged. Entry point: `index.ts`,
imported as `@mtgreplay/mtgr` (alias in `ui/vite.config.ts` and `ui/tsconfig.app.json`).

Peer fixes: none.

To update, re-copy `src/*.ts` and `LICENSE` from a newer commit, change the SHA here and
the version in `VENDORED_ASSETS` (`crates/release-tools/src/licences.rs`), then run
`cargo run -p release-tools -- licences`.
