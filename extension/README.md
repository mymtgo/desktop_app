# MyMTGO for Endstep (browser extension)

Captures Endstep traffic for MyMTGO. Manifest V3, Chrome and Firefox.

Needs Node 22.18 or newer (the build script imports a .ts file).

    npm install
    npm run build      # dist/chrome and dist/firefox (dev build)
    npm test && npm run check

Chrome: open chrome://extensions, turn on Developer mode, Load unpacked, pick `dist/chrome`.
The manifest carries a fixed dev key, so the extension id is stable.

Firefox: `npm run firefox` (runs `dist/firefox` through web-ext).

The repo root `.taurignore` lists `extension/` so `tauri dev` and `tauri build` keep using `ui/` as the frontend.

Before capturing:
- Start MyMTGO once. It registers the native helper the extension delivers to.
- After an update or a reload of the extension, it runs its relay again in open endstep.cc tabs, so capture continues without reloading the tab. After a fresh install, tabs that were already open have no page hook, so reload each one once. If the popup still shows Recording 0 in a tab opened before an update, reload the tab once. The relay also runs again every time the background script loads, so disabling and re-enabling the extension in Chrome needs no reload. If Firefox's permissions page has endstep.cc access switched off, the popup says so; allowing it again re-runs the relay.

The extension pings the helper when the browser starts, when the popup opens, when a unit is delivered and every 30 minutes (the `alarms` permission). The helper writes `last_seen.json` beside the inbox, and MyMTGO's Integrations page reads it to show whether the extension is connected.

## Store build and packages

    npm ci
    npm run build:store   # dist/store/chrome and dist/store/firefox
    npm run package       # the files to upload, in dist/packages

`npm run package` writes three zips: `mymtgo-endstep-chrome-<version>.zip`, `mymtgo-endstep-firefox-<version>.zip` and `mymtgo-endstep-source-<version>.zip`.

The store build differs from the dev build in three places. The Chrome manifest has no `key`, because the Chrome Web Store gives an upload its own extension id. The Firefox manifest declares `data_collection_permissions`, which Firefox Add-ons asks of every new listing, and needs Firefox 140 (the key is honoured from that version).

Version: `package.json` `version` is the extension's own, plain `major.minor.patch`. Bump it for every store upload. It does not follow the app's version.

The source zip packs the working tree (skipping dotfiles and dot folders), so package from a clean checkout: a stray untracked file under `src/` or `scripts/` would be published.

`PRIVACY.md` is the privacy policy the listings link to. `STORE_LISTING.md` holds the listing text and what the owner supplies.

### Publishing

Upload `mymtgo-endstep-chrome-<version>.zip` to the Chrome Web Store, and `mymtgo-endstep-firefox-<version>.zip` with `mymtgo-endstep-source-<version>.zip` to Firefox Add-ons. Two constants in the app follow the listings:

- `CHROME_EXTENSION_IDS` in `crates/endstep-capture/src/registration.rs`: add the store's Chrome extension id after the first upload, and release that app version before the Chrome listing goes public. Until then Chrome will not connect the store extension to the helper, and its popup says "MyMTGO helper not found".
- `STORE_LINKS` in `ui/src/lib/integrations.ts`: the listing addresses. Set, they replace the Integrations page's manual install steps with store buttons.

The full first-time list is `docs/superpowers/plans/2026-11-11-endstep-plan-5-owner-checklist.md`; the publishing steps are in the learning guide's Releasing chapter.

### Build note for Firefox reviewers

The source zip holds everything needed to rebuild the Firefox package. Build environment: any OS, Node 22.18 or newer (Node 24 works), npm 10 or newer. From the unpacked source zip:

    npm ci
    npm run package

The tools are open source: Vite builds each script into one plain file (no minifying, no source maps), and `web-ext build` zips the result. The extension code is in `dist/store/firefox` after the build, and the upload is `dist/packages/mymtgo-endstep-firefox-<version>.zip`.
