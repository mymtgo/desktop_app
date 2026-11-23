<script setup lang="ts">
import { LifeBuoy, PanelsTopLeft, Plug, Sparkles, TrendingUp, Wrench } from 'lucide-vue-next'

// What's new in 1.3: MyMTGO for Mac and Arena and Endstep sync between
// devices. It ships on edge as 1.3.0-rc.N first. Grouped, one sentence a point.

// Data, not markup: a long static list compiles to one static block, which
// the test renderer cannot mount.
const groups = [
  {
    name: 'New',
    icon: Sparkles,
    points: [
      'MyMTGO for Mac (Apple Silicon). It finds MTGO running under Wine (CrossOver, Whisky, Sikarugir, Kegworks or plain Wine) or, in a Windows VM, through the manual folder settings.',
      'On a Mac, overlays show on every Space, and the Endstep extension works in Chrome, Firefox, Chromium, Brave, Edge and Vivaldi.',
      'The first time you open the app on a Mac, macOS may ask you to allow it.',
      'Arena and Endstep sync across your devices, as MTGO already does: matches with their replays, decks and limited runs.',
      'A limited run played on more than one device, even one started on your phone, is joined into one run. If it is joined wrongly, move matches or set its status to fix it.',
      'Limited decks sync automatically for Supporters, on MTGO and Arena. Before, they never synced.',
      'A data console for power users (Settings, Advanced, Open data console): run your own read-only SQL over your matches, games, decks and card stats, chart the result and export it as CSV or JSON.',
    ],
  },
  {
    name: 'MTG Arena',
    icon: TrendingUp,
    points: [
      'Replays never show where an opponent\'s unrevealed cards were in their library, or which hidden card they put on the bottom.',
      'A card you searched for no longer shows a library position.',
      'Replays write attacks and blocks once they are submitted, name the permanent that charges an attack tax (for example Archangel of Tithes), and follow copies of spells.',
      'A match that ends without a result (a concede or an abandoned game) syncs straight away.',
      'You can set the status of an Arena limited run, so a run that closed wrongly can be reopened.',
      'Hiding or restoring a limited match updates its run.',
      'Accounts are listed by name. An account renamed before it signed in no longer shows twice.',
      'One damaged entry in the Arena log no longer loses a whole game state, and one unreadable stored event no longer stops tracking.',
      'Rank: the gaps you can claim on a manual match always match what is saved, and the rank chart\'s first icon no longer covers the class labels.',
      'Switching a manual match from Bo3 to Bo1 and back keeps games 2 and 3.',
    ],
  },
  {
    name: 'Endstep',
    icon: Plug,
    points: [
      'The Integrations page keeps the store links while Endstep is on and says matches appear after your first one.',
      'The status bar says when the extension is not connected.',
      'Match pages count mulligans, the starting hand size and discards more reliably.',
      'Cards seen only through an activated ability are listed, and double-faced cards count once.',
      'A ranked or casual match with no rating says Ranked or Casual in the Rank column.',
      'Replays show poison counters, untaps in the untap step and the back of double-faced cards.',
      'A replay\'s ending always names the same winner as the result.',
      'Importing is sturdier: repeated pages and results from Endstep, and a backlog of captures after the app was closed, no longer give wrong results.',
      'On Linux and Mac the helper is registered only for browsers you have installed.',
    ],
  },
  {
    name: 'Everywhere',
    icon: PanelsTopLeft,
    points: [
      'Switching between MTGO, Arena and Endstep leaves a page the new platform does not have.',
      'The platform switcher shows which platforms have a match in progress from the moment the app opens.',
      'A hidden match you show again appears straight away.',
      'A deck with no matches yet says "No matches with this deck yet".',
      'The replay window says whether a game is no longer stored or simply has no replay, and picking another game in it works when you go back to the first.',
      'An overlay window with only a sliver left on screen is moved back.',
      'Sync no longer downloads the same item on every sync when it cannot be used yet.',
      'When a new version has no build for your platform yet, the app says you are up to date instead of showing an update error.',
    ],
  },
  {
    name: 'Changes',
    icon: Wrench,
    points: [
      'Endstep browser extension 1.0.2: a Reset for the skipped count, a notice when Endstep access is off, a hint to reload a tab opened before the extension, and it keeps working after you turn it off and on in the browser.',
    ],
  },
  {
    name: 'Reporting problems',
    icon: LifeBuoy,
    points: [
      'If a match is missing or looks wrong on Mac, Arena or Endstep, tell us on Discord which match and when you played it.',
    ],
  },
]
</script>

<template>
  <header class="release-hero">
    <p class="release-eyebrow">MyMTGO 1.3</p>
    <h1 class="release-title">MyMTGO for Mac</h1>
    <p class="release-lede">
      MyMTGO now runs on Apple Silicon Macs, and your Arena and Endstep matches sync across your devices.
    </p>
  </header>

  <section v-for="group in groups" :key="group.name" class="release-group">
    <h3 class="release-group-name">
      <component :is="group.icon" :size="18" class="release-group-icon" aria-hidden="true" />
      {{ group.name }}
    </h3>
    <ul class="release-points">
      <li v-for="point in group.points" :key="point">{{ point }}</li>
    </ul>
  </section>
</template>

<style scoped>
.release-hero {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.75rem;
  padding: 2.5rem 0 1.5rem;
  text-align: center;
}

.release-title {
  font-family: 'Bungee', sans-serif;
  font-size: clamp(2.25rem, 6vw, 3.5rem);
  font-weight: 400;
  line-height: 1;
  letter-spacing: 0.02em;
  color: var(--color-foreground);
  text-shadow: 3px 3px 0 rgb(0 0 0 / 0.6);
}

.release-eyebrow {
  font-size: 0.8rem;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: #d9b77e;
}

.release-lede {
  max-width: 34rem;
  font-size: 0.95rem;
  line-height: 1.6;
}

.release-group {
  padding: 0.9rem 0;
  border-top: 1px solid var(--color-border);
}

.release-group-name {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding-top: 0;
  margin-bottom: 0.4rem;
  font-size: 0.875rem;
}

.release-group-icon {
  flex-shrink: 0;
  color: #d9b77e;
}

.release-points {
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
  padding-left: 1.85rem;
  list-style: disc;
}
</style>
