<script setup lang="ts">
import { LifeBuoy, PanelsTopLeft, Plug, Sparkles, TrendingUp, Wrench } from 'lucide-vue-next'

// What's new in 1.4: the event overlay for Challenges and other Swiss events,
// the win rate trend and MyMTGO Wrapped. It ships on edge as 1.4.0-rc.N first. Grouped, one
// sentence a point.

// Data, not markup: a long static list compiles to one static block, which
// the test renderer cannot mount.
const groups = [
  {
    name: 'New',
    icon: Sparkles,
    points: [
      'The event overlay covers Challenges, Showcase Challenges, Preliminaries and Qualifiers as well as leagues: round, record, matches left, the cut and your finish, on the desktop window and the OBS card.',
      'When a second event is active (a league match while you wait for the next Challenge round), it shows in a strip under the card. Turn it off in the overlay settings.',
      'The Overlays settings page has a live preview of the event card in every state, with the options under Customise.',
      'The Versus card shows your opponent\'s archetype once it is known, instead of always saying CHALLENGER.',
      'The win rate chart now shows each day you played (dots sized by matches) and a trend line that keeps moving even after thousands of matches.',
      'Recent sessions replaces the last 20 results: your last 7 days played, each with its record and win rate.',
      'Hovering the home chart lists the archetypes you played in that period.',
      'Interface scale (Settings, General) zooms the main window from 80% to 150%, for large or 4K screens.',
      'MyMTGO Wrapped arrives on 1 December: your year in Magic as a story of cards, from your deck of the year to your MVP card, across MTGO, Arena and Endstep. It needs 30 matches this year, and each card copies as an image to share.',
    ],
  },
  {
    name: 'MTG Arena',
    icon: TrendingUp,
    points: [
      'Spark rank, below Bronze, is tracked with its own icons, metagame band and rank chart row.',
      'Renaming your Arena account no longer loses rank readings or leaves a second account behind.',
    ],
  },
  {
    name: 'Endstep',
    icon: Plug,
    points: [
      'Long matches that end on the clock now reach MyMTGO once your Endstep browser extension updates.',
      'The placeholder deck a match used before its real deck was known is archived when it is no longer needed. Restore brings it back.',
    ],
  },
  {
    name: 'Everywhere',
    icon: PanelsTopLeft,
    points: [
      'Chart colours follow one rule: green and red mean results, your own lines are blue, and comparison lines are grey and dashed.',
      'The Cards page says what it lists.',
      'MyMTGO\'s source code is published with each release at github.com/mymtgo/desktop_app.',
    ],
  },
  {
    name: 'Changes',
    icon: Wrench,
    points: [
      'The Classic event overlay window is now exactly the size of the card, so the card sits slightly up and left of where it was. Move the window back if you need to.',
      'The cumulative win rate line is gone; the trend line replaces it.',
      'MyMTGO now counts anonymous usage (which pages and features are used, never your names, decks or matches). Turn it off with Share anonymous usage data in Settings; offline mode always stops it.',
    ],
  },
  {
    name: 'Reporting problems',
    icon: LifeBuoy,
    points: [
      'If the event overlay shows the wrong round, record or finish in a Challenge or Qualifier, tell us on Discord which event and when you played it.',
    ],
  },
]</script>

<template>
  <header class="release-hero">
    <p class="release-eyebrow">MyMTGO 1.4</p>
    <h1 class="release-title">Event overlay</h1>
    <p class="release-lede">
      The overlay now follows Challenges and Qualifiers through Swiss and the top cut, and your win rate shows a trend you can read.
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
