# Privacy policy: MyMTGO for Endstep

MyMTGO for Endstep is a browser extension for people who play on endstep.cc and use the MyMTGO desktop app. It copies your own Endstep matches to MyMTGO on your computer.

## What it reads

The extension runs only on pages at https://endstep.cc. It reads:

- The websocket messages your browser exchanges with Endstep while you play and while you are in the lobby.
- The responses to a short list of Endstep web requests: matches, decks, lobbies, ranked, queue, tournaments, drafts, cubes, and the signed-in user's account status and live status (the site's own `/api/auth/me` and `/api/me/live` responses, with the email removed).

It reads what your own page already receives and sends. This includes your player name and your opponents' player names as Endstep shows them, because MyMTGO needs them to list your matches.

## What it removes before saving

The extension removes secrets from every JSON message before it is stored. Endstep sends JSON; a message that is not JSON would be stored as received. It removes:

- Session tickets and tokens.
- Cookie and authorization values.
- Passwords, secrets, email addresses and similar fields.
- The query string of every recorded web address, which can carry a token.

## What the extension declares to Firefox

Firefox Add-ons asks each extension to declare what it collects. This one declares website content (`websiteContent`), because it reads the pages' messages and hands them to an app on the same computer.

## Where the data goes

Only to the MyMTGO helper program on the same computer. The extension hands each finished capture to the helper through the browser's native messaging. A capture is either one match, or one lobby session: the lobby and web request traffic outside a match, kept in rolling pieces of up to 30 minutes. Lobby sessions can include other players' names, as Endstep shows them. The helper saves each capture as a file in the inbox folder beside it, in MyMTGO's folder on your disk. MyMTGO reads those files and builds your match history. MyMTGO keeps the captures in its own database on your computer.

While a match or a lobby session is open, and until the helper confirms the file is saved, the extension keeps it in the browser's own extension storage. It then deletes its copy.

About every 30 minutes, and when the browser starts, when the popup opens and when a capture is delivered, the extension pings the helper. The ping tells the helper the browser name and the extension version. The helper writes them, with the time, to a small file beside the inbox, which MyMTGO's Integrations page reads to show whether the extension is connected.

When you switch Endstep off in MyMTGO, MyMTGO removes the helper's registration with your browsers and stops reading the inbox. The extension then cannot reach the helper, so it keeps what it captured in the browser's storage until you switch Endstep on again. Files already in the inbox stay there and are not imported until the switch is on.

## What it never does

- It has no server. It sends nothing to the developer or to anyone else.
- It has no analytics, tracking, advertising or crash reporting.
- It does not sell or share data.
- It does not change Endstep. It does not click, edit or add messages, or make requests of its own to Endstep.
- It does not keep matches against the Forge AI or matches you only watch. The extension writes such a match to its browser storage while it runs, like any other, and deletes it when the match ends. A match the extension cannot classify is handed to MyMTGO. MyMTGO keeps the capture with the others and does not show it as a match when it finds a bot or that you were not a player.

The MyMTGO desktop app has its own behaviour for what it sends to MyMTGO services. For Endstep matches, it sends card lookups (card names and printings) and the deck archetype estimate (format, card names and quantities, no player names). If you sign in to MyMTGO in the app, it uploads your Endstep decks to your own MyMTGO account. For a deck you turn sync on for, it also uploads its matches, including the captured match data replays are built from, your Endstep username and account ID, and your opponents' usernames and account IDs, so your other computers can show them.

## How to remove your data

- Turn off "Capture Endstep matches" in the extension's popup to stop capture.
- Turn off the Endstep switch in MyMTGO's Integrations page to stop MyMTGO reading captured files.
- Uninstall the extension to remove its stored data from the browser.
- MyMTGO keeps the captures in its own database on your computer. Switching Endstep off in MyMTGO stops the import but keeps what is stored. Deleting a match in MyMTGO does not remove the stored captures, including captures of matches MyMTGO did not show. Uninstalling MyMTGO and deleting its data folder removes them.

## Changes and contact

If this policy changes, the new version is published at the same address with a new date. Questions: use the contact address on the MyMTGO website, https://mymtgo.com.

Last updated: 2026-11-19.
