# Store listing text

Used for the Chrome Web Store and Firefox Add-ons. The name matches the manifest `name`: `MyMTGO for Endstep`. The extension version is in `package.json` (see README.md).

## Name

MyMTGO for Endstep

## Short description

Chrome allows 132 characters for the summary (the manifest `description` limit) and takes the summary from the manifest `description`, which is set to this text in `src/manifest.ts` (a test keeps it within 132 characters and the same in both builds). This text is 66 characters and works for Firefox Add-ons too.

Saves your own Endstep matches to the MyMTGO app on your computer.

## Full description

MyMTGO for Endstep copies your own Endstep matches to MyMTGO, the Magic: The Gathering match tracker for your desktop.

Install MyMTGO, turn on Endstep in its Integrations page, then play on endstep.cc as usual. Your matches, decks, opponents and replays appear in MyMTGO. If endstep.cc was already open when you added the extension, reload that tab once.

How it works:
- It runs only on endstep.cc.
- It copies the messages your browser already exchanges with Endstep.
- It removes session tickets, tokens and secret fields before saving.
- It hands each finished match you play, and lobby activity, to the MyMTGO helper on your computer. Nothing goes to a server.
- It never clicks, edits or adds anything on Endstep.

You need the MyMTGO desktop app for Windows or Linux. Start MyMTGO once so it can set up the helper. This extension does nothing useful without it.

Privacy policy: see the link in the listing's privacy field.

## Category

Chrome Web Store: Fun, or Productivity if Fun is not offered for extensions (owner picks).
Firefox Add-ons: Games and entertainment, and Other (up to 2).

## Single purpose (Chrome privacy practices tab)

Copies the user's own Endstep matches from endstep.cc to the MyMTGO desktop app on the same computer.

## Permission justifications (Chrome privacy practices tab)

- `nativeMessaging`: Sends each captured match to the MyMTGO helper program on the user's computer. This is the only place the data goes.
- `storage`: Keeps the capture on/off setting and the status shown in the popup.
- `unlimitedStorage`: Keeps a match in the browser's extension storage while it is open and until the helper confirms it is saved. A long match can be several megabytes.
- `alarms`: A repeating alarm wakes the extension about every 30 minutes to ping the helper, so MyMTGO can show that the extension is connected. The ping carries the browser name and extension version. The alarm itself only pings the helper. If it wakes a stopped background worker, the worker's start-up pass also sends waiting captures. They are also sent when a capture closes.
- `scripting`: Starts the extension's relay script again in endstep.cc tabs that are already open: after an install or update, every time the background script loads (so a disable then enable in the browser does not leave a tab without it) and when endstep.cc access is granted again. A match in progress is therefore not lost. When the popup opens, it also runs one small function in the active endstep.cc tab's page that reads a single property, to learn whether the capture hook is present, and changes nothing. After a fresh install, a tab that was already open has no page hook and needs one reload.
- Host permission `https://endstep.cc/*`: The capture scripts run on endstep.cc pages, and `scripting` needs this host to reach open tabs. No other site is matched.

Remote code: none. All code is in the package.

Data usage disclosures (Chrome, as submitted 2026-11-12): personally identifiable information (player names), user activity (the page's network traffic, including the user's own game actions) and website content, handled only on the user's computer. Certify that data is not sold, not used for unrelated purposes and not used for creditworthiness. The owner confirms the exact checkboxes in the dashboard.

## Firefox Add-ons notes

- The manifest declares `data_collection_permissions` with `required: ["websiteContent"]` (see the confirm list below).
- Source code: the build bundles each script with Vite, so Firefox Add-ons wants the source. Upload `mymtgo-endstep-source-<version>.zip`. Its build note is in README.md ("Build note for Firefox reviewers").
- Reviewer note to paste: "The extension copies endstep.cc websocket and REST data to a local native messaging host (com.mymtgo.endstep, installed by the MyMTGO desktop app). It makes no network requests of its own."

## What the owner supplies

- A Chrome Web Store developer account (one-time fee) and a Firefox Add-ons developer account.
- Screenshots. Chrome needs at least one at 1280x800, up to 5. Suggested: the popup with a green helper dot, and the match in MyMTGO.
- A privacy policy URL where `extension/PRIVACY.md` is published (for example a page on the MyMTGO website). Both stores ask for it.
- The Chrome store icon (128x128) is `icons/icon-128.png`.
- The Chrome Web Store extension id after the first upload. The helper's registration lists the Chrome extension ids it accepts, so add the store id there (spec section 4.8).
- The files to upload: the three zips from `npm run package`.

## For the owner to confirm before submitting

These were not settled by the vendor documentation read on 2026-11-11.

1. Firefox `data_collection_permissions`. Source: https://extensionworkshop.com/documentation/develop/firefox-builtin-data-consent/ . It says the key lives at `browser_specific_settings.gecko.data_collection_permissions`, takes `required` and `optional` lists, and that an extension that collects nothing declares `"required": ["none"]`. It does not say whether handing website content to a local native application counts as collection. The chosen declaration is `{ "required": ["websiteContent"] }`, the cautious reading. If Mozilla treats a local native host as no transmission, `{ "required": ["none"] }` is the alternative. Unverified. The page says the key is required for extensions created from 2025-11-03.
2. Chrome `key` in an upload. https://developer.chrome.com/docs/extensions/reference/manifest/key describes `key` as a way to keep the same id during development and says nothing about uploads. The store build omits it, which is safe either way. Unverified whether the dashboard would accept it.
3. Listing field limits. https://developer.chrome.com/docs/webstore/cws-dashboard-listing gave no limits for name or description. The manifest `description` limit of 132 characters is from https://developer.chrome.com/docs/extensions/reference/manifest/description . Firefox Add-ons limits were not on https://extensionworkshop.com/documentation/publish/submitting-an-add-on/ . The short description above is short enough for 132. Check the name and full description limits in each dashboard.
4. Category names for both stores were not in the pages read. Pick what the dashboard offers.
5. Chrome data usage checkboxes. https://developer.chrome.com/docs/webstore/cws-dashboard-privacy says to disclose data types and certify each statement, without listing the types. Choose them in the dashboard to match PRIVACY.md.
6. Firefox minimum version. The store Firefox build sets `strict_min_version` to 140.0 (dev builds keep 128.0). Source: https://extensionworkshop.com/documentation/develop/firefox-builtin-data-consent/ says "Firefox supports built-in consent for data collection and transmission in Firefox for desktop 140 and later" and suggests 140 for desktop (and 142 for Firefox for Android, `gecko_android`; this extension declares no Android support). Verified for desktop 140.
7. Decide the two stores' data disclosures together. Chrome: "website content" and "personally identifiable information (player names)". Firefox: whether to also declare `personallyIdentifyingInfo` and `websiteActivity` (the user's own game actions are captured), beyond `websiteContent`. Keep PRIVACY.md and both dashboards saying the same thing.
8. Chrome category: this file says "Fun", which may be an outdated name. Confirm the category names in the dashboard.
9. Listing limits as known, all unverified unless noted: Chrome name 75 characters, summary 132 (the 132 is documented, see item 3); Firefox name 50, summary 250. Check each in the dashboard.
10. The privacy contact line in PRIVACY.md points to https://mymtgo.com. Replace it with a real contact address if there is one.
