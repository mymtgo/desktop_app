<script setup lang="ts">
/**
 * The v1 Data & Privacy disclosure (#264): a plain-language account of what
 * the app keeps on this computer, what it sends and when, and what offline
 * mode changes. Written from v1's code; it replaces 0.x's text and keeps
 * everything 0.x promised that is still true. Rendered beneath the offline
 * mode switch so the tradeoff is visible before anyone flips it.
 */
</script>

<template>
  <div class="flex flex-col gap-3 rounded-md border border-border bg-muted/30 p-3">
    <p class="text-xs font-semibold text-foreground">What data is kept and sent</p>

    <div class="max-h-64 overflow-y-auto pr-1">
      <div class="flex flex-col gap-3 text-xs text-muted-foreground">
        <section class="flex flex-col gap-1">
          <p class="font-medium text-foreground">Kept on this computer</p>
          <p>
            Everything the app records is stored in a database on this computer: your matches, games, decks, leagues, drafts, notes,
            archetypes and settings. It is built by reading files MTGO writes here: mtgo.log, your deck files, MTGO's game log files, its
            match history files when you import them, and its card files for token names. A MyMTGO 0.x database is only read, never
            changed, when you import it.
          </p>
          <p>
            The folders you choose, the app's own log files (one per day, the last 7 kept) and any card images you download also stay
            here. The app's log files can contain MTGO usernames, yours and your opponents'. Your sign-in and this device's API key are
            kept in the system keychain or, on Linux without one, in an obscured file in the app's data folder, which is not real
            encryption.
          </p>
        </section>

        <section class="flex flex-col gap-1">
          <p class="font-medium text-foreground">Sent when offline mode is off</p>
          <p>
            When your deck's archetype cannot be confidently detected on this computer, its decklist and format are sent to the API for
            an estimate. The app also checks for updates to the archetypes it holds by sending their IDs and versions, and asks for
            metagame shares and archetype decklists when the Metagame page, an archetype or the game overlay needs them.
          </p>
          <p>
            With the game overlay on (it is off by default), the app sends your current opponent's MTGO username and the
            format to look up their known archetype, and may send the cards they have revealed so far for an estimate. This feature
            sends a lookup as well as receiving one.
          </p>
          <p>
            After each match, the app sends your MTGO username and login ID, the match result, the format and a timestamp, the app
            version, your decklist and its detected archetype, your opponent's MTGO username and detected archetype, and any league or
            tournament token and round number. For every game it also sends the turn count, start and end times and your opening hand:
            the exact cards you kept, how many hands you mulliganed, whether you were on the play, and your opponent's mulligan count.
            Matches you enter by hand are not reported.
          </p>
          <p>
            MTG Arena matches are reported the same way, to the same place: your Arena username (such as Name#12345) and Arena user
            ID, the result, format, timestamp, app version, your decklist by Arena card ID and its archetype, your opponent's Arena name,
            and the same per game data. An Arena report also carries the event name, whether it was best of one or best of three, the
            match type (such as limited), the season, and your rank when it was last seen: ladder, rank and tier, step, Mythic
            percentile or placement, and when it was observed. For a draft or sealed run it carries the run's token and number, and for
            a Brawl match both players' commanders. Endstep matches are not reported.
          </p>
          <p>
            Your opponent's decklist goes with both kinds of report: the cards the app saw them use, up to four copies of each. This is another
            player's data rather than yours, and it is submitted so the community archetype database stays current.
          </p>
          <p>
            Tournament observations are sent for any MTGO tournament event your client syncs, including events you only browse in the
            lobby rather than enter. Most of them carry a player roster for that event, containing other players' MTGO usernames, numeric
            login IDs and avatar IDs. Round by round they also carry everyone's standings (rank and points), elimination status, and the
            round's match pairings, including a match ID and token for every game in the round, exactly as MTGO broadcasts them rather
            than only the games you played. These are MTGO's own messages passed through unfiltered, so they can contain more than the
            list above, including a human readable event name that MTGO puts in the same raw message. As with the opponent decklist, this
            is other players' data rather than yours.
          </p>
          <p>
            A closing report is sent when an event ends, carrying one further MTGO login ID, which may identify another competitor or
            your own account.
          </p>
          <p>
            For MTGO matches only, per game, per card statistics are sent with your deck's archetype and the archetype they were observed against, the format,
            the result, whether you were on the play, whether it was a sideboarded game and the date. They carry no username or match ID.
            To show a tournament's real name, the app sends its MTGO event ID; the name is looked up from mymtgo's own tournament
            records, not read back from what you sent.
          </p>
          <p>
            None of this needs a MyMTGO account: signed out, it goes with this device's identifier (see below). Matches played while
            offline mode was on are sent once you turn it off.
          </p>
          <p>
            Anonymous usage counts go to Aptabase, an analytics service hosted in the EU, unless you turn off "Share anonymous usage
            data": that the app started, which pages you open and which features you use (such as copying an image), with your
            operating system, its version, your language and the app version. They carry no account, username, device identifier,
            deck, card, match or file name.
          </p>
        </section>

        <section class="flex flex-col gap-1">
          <p class="font-medium text-foreground">Sent only while signed in</p>
          <p>
            Signing in opens mymtgo.com in your browser, which hands the result back to the app on this computer only. Cloud sync then
            runs every 30 minutes and shortly after each match, and downloads what your other devices have synced. It uploads your
            decks that are not archived (draft and sealed decks only with Supporter): their names, formats, every version's decklist,
            sideboard guides, matchup notes and archetype. For the decks you choose to sync, it also uploads their matches and leagues:
            results, games and timings, the cards each side played and saw, opening hands and mulligans, the game states replays are
            built from, your notes, your opponent's MTGO username and, for leagues, your drafts and picks.
          </p>
          <p>
            MTG Arena and Endstep decks are uploaded the same way. For the ones you choose to sync, their matches and Arena draft and
            sealed runs are uploaded too, with the raw game data replays are built from (the Arena log messages or Endstep capture of
            each match), and the usernames and account IDs in it: your Arena or Endstep username and account ID, and your
            opponent's.
          </p>
          <p>
            The app also tells MyMTGO which MTGO accounts (login ID and username) it has seen on this computer, so synced matches show
            under the right player.
          </p>
          <p>
            Sharing a replay, when you choose Share, uploads that match's game states, game log and sideboarding, its format, date and
            archetypes, and your MTGO login ID, with each opponent's name replaced by "Opponent". Your own username stays in it, and
            in the link. The replay opens on an unlisted page that anyone with the link can watch, without signing in; search engines
            are asked not to list it. You can switch the link off later, and it stops working within a few minutes.
          </p>
          <p>
            With "Publish live overlay" on, the app sends your league card for your current MTGO account: the event name and format,
            your record, the current match's game results, your deck's name, archetype and label, and a link to its art, plus your custom
            background image if you set one. It is sent on each change and every 30 seconds. Turning it off asks MyMTGO to remove it.
          </p>
        </section>

        <section class="flex flex-col gap-1">
          <p class="font-medium text-foreground">Sent always, even in offline mode</p>
          <p>
            MTGO card identifiers and card or token names, looked up against the API to get card names, mana costs and an image link so
            your decks keep rendering. No match, result or player data is attached to these lookups.
          </p>
          <p>
            Signed out, every request above, card lookups included, carries a generated device identifier and a short lived API key the
            app gets by registering that identifier. Registration keeps happening while offline mode is on because card lookups depend on
            it. Signed in, requests carry your account's sign-in token instead, so the API knows which account sent them.
          </p>
        </section>

        <section class="flex flex-col gap-1">
          <p class="font-medium text-foreground">Sent in offline mode only when you ask</p>
          <p>
            Download archetypes, on the offline mode card. It asks for the list of active archetypes and their definitions once,
            signed with this device's identifier and API key only, never your sign-in, and sends nothing about your matches, decks or
            account. The archetypes are kept on this computer as Custom archetypes you can edit. When you turn offline mode off they
            become MyMTGO's archetypes again and your edits to them are dropped.
          </p>
        </section>

        <section class="flex flex-col gap-1">
          <p class="font-medium text-foreground">Not affected by offline mode</p>
          <p>
            App updates. The app checks releases.mymtgo.com shortly after it starts and every hour, downloads a newer version in the
            background and installs it only when you choose. The check carries no account or device identifier; the server sees your IP
            address and whether you are on the stable or edge channel. Offline mode never blocks updates.
          </p>
          <p>
            Error reporting. Crash and error reports go to Sentry, a third party error tracking service, whatever offline mode is set to.
            There is no setting to turn them off. A report holds the error and where in the app it happened, the app version, your
            operating system and its version, your processor type and recent app log messages. Before a report is sent, the MTGO
            usernames the app knows or finds in it are replaced with "&lt;player&gt;" and your home folder with "~", and your computer's
            name and user details are left out. A report that cannot be cleaned this way is not sent. MyMTGO's error reporting service
            does not store your IP address.
          </p>
          <p>
            Card images. Once a card's image link is known, the app loads the image from Scryfall's servers rather than ours. Scryfall is
            a third party, and it sees your IP address and, in near real time, which cards you are looking at. This is outside our
            control. Turning on "Download card images locally" in Settings avoids it after the first download by keeping the images on
            your computer instead.
          </p>
        </section>

        <section class="flex flex-col gap-1">
          <p class="font-medium text-foreground">Only when you ask</p>
          <p>
            "Download report data" in Get help saves a zip file to a place you choose; nothing is uploaded. It holds the newest mtgo.log,
            the newest MTG Arena Player.log, the three newest MyMTGO log files and a summary (app version, update channel, operating system, watcher state, offline mode,
            whether you are signed in, and match and game counts). The logs are not cleaned, so they contain MTGO and Arena usernames,
            yours and your opponents'. It never holds the database, settings, deck files or sign-in details.
          </p>
        </section>

        <section class="flex flex-col gap-1">
          <p class="font-medium text-foreground">What offline mode gives up</p>
          <p>
            Archetype estimates and updates, metagame shares, opponent scouting, tournament names, cloud sync, replay sharing and the
            published overlay all rely on the traffic above, so expect them to go stale or become unavailable while offline mode is on.
            After you turn offline mode off, it cannot be turned on again until the next day.
          </p>
        </section>

        <section class="flex flex-col gap-1">
          <p class="font-medium text-foreground">Deleting your data</p>
          <p>
            Signing out removes your sign-in from this computer and stops cloud sync, replay sharing and the published overlay. It does
            not delete what MyMTGO already holds, and it does not stop match reports; offline mode does. "Delete all imported" on the
            Import page removes the matches imported from MTGO's history files. Turning off "Download card images locally" deletes the
            downloaded images.
          </p>
          <p>
            To remove everything from this computer, sign out, then uninstall. On Windows, tick "Delete the application data" in the
            uninstaller, or the data folder (%APPDATA%\com.mymtgo.desktop) stays. On Linux, delete ~/.local/share/com.mymtgo.desktop
            yourself. This device's API key stays in the system keychain after uninstalling; to remove it, delete the
            com.mymtgo.desktop entries in Credential Manager on Windows or in your keyring on Linux.
          </p>
        </section>
      </div>
    </div>

    <p class="text-xs text-foreground">Leaving offline mode switched off means you agree to your data being sent as described above.</p>

    <p class="text-xs text-muted-foreground/70 italic">Last updated: 25 November 2026</p>
  </div>
</template>
