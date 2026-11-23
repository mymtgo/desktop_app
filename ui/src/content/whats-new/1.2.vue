<script setup lang="ts">
import { LifeBuoy, PanelsTopLeft, Sparkles, TrendingUp, Wrench } from 'lucide-vue-next'
import { RouterLink } from 'vue-router'
import { Button } from '@/components/ui/button'
import arenaLogo from '@/assets/platforms/arena-logo.png'
import endstepLogo from '@/assets/platforms/endstep-logo.png'

// What's new in 1.2: MTG Arena support (#530: plans 1 to 6, #539 to #606),
// the Arena tweaks (#608) and the report's Arena log. It ships on edge as
// 1.2.0-rc.N first, so the lede says Arena is new and how to report
// problems. Grouped, one sentence a point. Add a Fixes group with the 1.2
// patches when there are some (rc.2 added one, rc.3 extended it). rc.4 adds the
// opt-in MTGO recorder (#639). rc.6 adds custom deck covers (#646), Arena
// match reports (#723), archetype trends (#729) and Arena Brawl (#739).
// rc.8 adds Endstep (#647, #715) and the Integrations page. The hero leads
// with the two new platforms and a button to Settings, Integrations, where
// both are switched on. 1.2.0 adds cover cropping, the league entry fix
// (#757), Arena Edit match games (#573, #582), Mythic never rounded up
// (#555) and archetypes prefilled for Arena and Endstep decks (#690).
// 1.2.1 is #569 (#760), #759 (#761) and #677 (#763).

// Data, not markup: a long static list compiles to one static block, which
// the test renderer cannot mount.
const groups = [
  {
    name: 'New',
    icon: Sparkles,
    points: [
      'MTG Arena: your Bo1 and Bo3 matches are tracked from Arena\'s log while you play, beside your MTGO matches.',
      'Switch between MTGO and Arena with the logos at the top: each page shows that platform\'s matches, decks, opponents and cards.',
      'Rank over the season: a rank chart, constructed and limited rank on the dashboard, and the rank each deck reached.',
      'Arena match pages have the game log, and Card Stats, Game Stats and Sideboard guides work for Arena decks.',
      'Arena archetypes, and an Arena metagame by month and rank band.',
      'Replays of your Arena games.',
      'A deck that played both Bo3 and Bo1 gets a Bo3 | Bo1 switch: its pages show one kind at a time, and Bo1 hides the sideboard columns and pages. On the Decks list, a card shows its record over every match with a tag for each kind played.',
      'Add an Arena match the app did not see by hand, so the rank it won or lost counts for that deck.',
      'Arena drafts and sealed events, Arena Directs included, show on the Limited pages like MTGO leagues: your record, card pool, deck and matches.',
      'A Trends tab under each archetype on the Decks page charts its win rate over time, as one line or a line for each deck.',
      'Upload your own image as a deck\'s cover, instead of only picking a card. A large image can be cropped to fit.',
      'Arena Brawl and Competitive Brawl matches are tracked. The commander is the archetype (for both you and your opponent), and a Brawl deck\'s page shows its Commander section first.',
      'Endstep: your endstep.cc matches are tracked through the MyMTGO for Endstep browser extension (Chrome, Edge, Brave and Firefox), with decks, opponents, hands, game logs and replays.',
      'A new Integrations page in Settings switches Arena and Endstep on or off, and links to the Endstep extension in your browser\'s store.',
      'Optional MTGO replays on Windows: Settings, Replays downloads a separate recorder that saves a full replay of each match. It reads the MTGO client, so read its risk notice before you turn it on.',
    ],
  },
  {
    name: 'Improvements',
    icon: TrendingUp,
    points: [
      'The status bar and the File Paths settings follow the platform you are looking at.',
      'Pick the metagame\'s month from a dropdown.',
    ],
  },
  {
    name: 'Changes',
    icon: PanelsTopLeft,
    points: [
      'Overlays stay MTGO only for now.',
      'On a new install, Arena starts switched off: turn it on in Settings, Integrations. If the app has already seen Arena, it stays on.',
      'Your finished Arena matches are now sent to MyMTGO for the metagame, as MTGO matches are. They still do not sync between your computers.',
      'Card images are always stored on your computer, so pages work offline and load faster. Settings now only chooses the folder.',
    ],
  },
  {
    name: 'Fixes',
    icon: Wrench,
    points: [
      'Cards added recently showed their name instead of their image, and broke deck screenshots.',
      'Arena accounts whose name has no #tag are tracked.',
      'On an archetype\'s tabs, the win rate, record and best and worst matchups at the top follow the timeframe you pick.',
      'Arena Directs and other limited events played before 1.2.0-rc.2 showed as one run: each entry now has its own run.',
      'Right after starting the app or waking your computer, Settings could show "The last sync run failed" for half an hour. A sync that cannot connect is now tried again within minutes, and the error only shows if it keeps failing.',
      'Decks you deleted no longer count as waiting in the cloud.',
      'MTGO sealed leagues ran to six matches and took the first match of your next entry. A league run now ends at that league\'s length (five for Competitive Sealed), and entering a league again starts a new run. Your past runs are regrouped after the update.',
      'Arena and Endstep decks get an archetype guessed from their cards, as MTGO decks do.',
      'Adding a game to an Arena match with Edit match now fills in that game\'s cards, and the match gets its archetype.',
      'Mythic percentiles are never rounded up: 99.96% shows as Mythic 99.9%.',
      '1.2.1: in MTGO card stats, a hand you sent back with a mulligan no longer counts its cards as seen. This applies to matches played from 1.2.1 on.',
      '1.2.1: an Arena match is sent to MyMTGO for the metagame once its archetype is detected, so it arrives with the archetype.',
      '1.2.1: the live dot in the header shows only on the platform where a match is in progress, not on every other logo.',
    ],
  },
  {
    name: 'Reporting problems',
    icon: LifeBuoy,
    points: [
      'Get help, Download report data now includes Arena\'s log: share the zip on Discord if anything about Arena looks wrong.',
      'Endstep is new too: if a match is missing or looks wrong, tell us on Discord which match and when you played it.',
    ],
  },
]
</script>

<template>
  <header class="release-hero">
    <div class="release-platforms">
      <img :src="arenaLogo" alt="Magic: The Gathering Arena" class="release-logo release-logo-arena" />
      <span class="release-platform-plus" aria-hidden="true">+</span>
      <img :src="endstepLogo" alt="Endstep" class="release-logo release-logo-endstep" />
    </div>
    <p class="release-eyebrow">MyMTGO 1.2</p>
    <h1 class="release-title">MyMTGO has evolved</h1>
    <p class="release-lede">
      MyMTGO now tracks your MTG Arena and Endstep matches too, right beside your MTGO ones: decks, opponents, archetypes
      and replays for every platform you play on.
    </p>
    <Button as-child size="lg" class="release-cta">
      <RouterLink to="/settings/integrations">Set up Arena and Endstep</RouterLink>
    </Button>
    <p class="release-cta-hint">Endstep needs the MyMTGO for Endstep browser extension; the Integrations page links to it.</p>
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

.release-platforms {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 1.75rem;
  margin-bottom: 0.75rem;
}

.release-logo {
  width: auto;
  /* The page's prose styles give every img full width and a border. */
  border: 0;
  border-radius: 0;
}

/* The Arena logo is the official artwork (owner supplied); the Endstep one is
   its mark and ENDSTEP in Inter at weight 650 and 0.16em tracking, as
   endstep.cc's .es-brand-word, drawn white on transparent. */
.release-logo-arena {
  height: 5.5rem;
}

.release-logo-endstep {
  height: 2.6rem;
}

.release-platform-plus {
  font-size: 1.75rem;
  color: #d9b77e;
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

/* The app's bevelled Button; the prose styles would otherwise underline the link and colour it. */
.release-cta {
  margin-top: 0.5rem;
  color: var(--color-primary-foreground);
  text-decoration: none;
}

.release-cta-hint {
  font-size: 0.8rem;
  color: var(--color-muted-foreground);
}

@media (max-width: 30rem) {
  .release-platforms {
    flex-direction: column;
    gap: 0.75rem;
  }
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
