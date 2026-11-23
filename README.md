# MyMTGO desktop app

MyMTGO tracks your Magic: The Gathering matches on your own computer: decks, leagues, drafts, opponents, archetypes, replays and stream overlays. It reads the files the game clients already write. It never reads game memory and needs no game API.

- **Magic: The Gathering Online:** MTGO's `mtgo.log` and deck files.
- **MTG Arena:** Arena's `Player.log`.
- **Endstep:** a browser extension (`extension/`) hands captured matches to the app.

Downloads, accounts and the website are at [mymtgo.com](https://mymtgo.com). This repository holds the app's source, published so anyone can read it, check what it does with their data, and build it for themselves. It is a read-only copy, refreshed at each release.

## Licence

The source is **source-available, not open source**. See [LICENSE](LICENSE): you may read it, build it and run it for your own personal, non-commercial use. You may not redistribute it, in source or binary form, or publish a modified build. Forks on GitHub are fine for reading and building; please do not publish builds from them.

## Building

The app is Rust ([Tauri 2](https://tauri.app)) with a Vue UI.

1. Install [Rust](https://rustup.rs) (the toolchain in `rust-toolchain.toml` installs itself), [Node.js](https://nodejs.org) 22 or newer and [pnpm](https://pnpm.io) 10.
2. Install Tauri's system dependencies for your platform: see [Tauri's prerequisites](https://tauri.app/start/prerequisites/) (WebKitGTK on Linux, the MSVC build tools on Windows, the Xcode command line tools on macOS).
3. From the repository root:

   ```sh
   pnpm --dir ui install
   ui/node_modules/.bin/tauri dev
   ```

   Run it from the repository root, not from `ui/`.

The tests:

```sh
cargo test --workspace
pnpm --dir ui test
```

### The MyMTGO API

A development build talks to no server unless you tell it to, so tracking works fully offline. Sign-in, cross-device sync, archetype lookups and match reports need the MyMTGO API and an OAuth client id that only MyMTGO issues. To point a development build at an API, create `.env.local` at the repository root:

```sh
MYMTGO_API_URL=https://mymtgo.com
```

then run `touch app/build.rs` so the next build picks it up.

## Finding your way around

- `crates/`: the Rust workspace. Pure crates (parsing, projection, rules) never touch the disk, network or clock; `store` is the SQLite database; `tracker` is the running pipeline.
- `app/`: the Tauri shell (windows, commands, the API client, overlays).
- `ui/`: the Vue app.
- `extension/`: the Endstep browser extension (see its README).
- `fixtures/`: anonymised real game logs the tests run against. Player names and account ids in them are fakes.
- `docs/known_mtgo_lies_and_traps.md`: the log quirks every parser has to survive.

## Issues

Bug reports and ideas are welcome in [Issues](https://github.com/mymtgo/desktop_app/issues). This repository does not take pull requests. Issues are public, so please do not attach game logs or the app's Get help report: they contain other players' names. If we need the report, we will ask you to send it privately.
