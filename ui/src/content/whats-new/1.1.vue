<script setup lang="ts">
import { PanelsTopLeft, Sparkles, TrendingUp, Wrench } from 'lucide-vue-next'

// What's new in 1.1: the offline archetype download (#517, #518), bulk
// Detect with live progress (#520), the Custom badge (#522, #524), the deck
// right-click menu (#525) and the restyled match page (#519). The
// offline points were drafted for 1.0.5, which never shipped on its own.
// Grouped, one sentence a point. Fixes lists what each 1.1 patch fixed:
// 1.1.1 is #526, #528 and #558; 1.1.2 is #611.

// Data, not markup: a long static list compiles to one static block, which
// the test renderer cannot mount.
const groups = [
  {
    name: 'New',
    icon: Sparkles,
    points: [
      'Archetypes offline: download every archetype once from the offline mode card, so detection works without going online.',
      'Detect archetypes: select matches in any matches table and detect them all at once, each row filling in as it is done.',
      'Right-click a deck on the Decks page to set or clear its archetype without opening the deck.',
    ],
  },
  {
    name: 'Improvements',
    icon: TrendingUp,
    points: [
      'Archetypes you made yourself carry a Custom badge, so they are easy to tell apart from MyMTGO archetypes with the same name.',
      'Detection you ask for runs faster than before.',
    ],
  },
  {
    name: 'Changes',
    icon: PanelsTopLeft,
    points: [
      'The match page uses the same raised panels as the rest of the app, with a tile for each game.',
    ],
  },
  {
    name: 'Fixes',
    icon: Wrench,
    points: [
      '1.1.1: a new deck gets its archetype once its maindeck is complete, not from the first few cards added while you build it.',
      '1.1.1: when you log in after a tournament round has started, the round is linked to your deck from game 1, so the overlay shows draw odds straight away.',
      '1.1.1: an archetype\'s Matchups and Card Stats tables scroll again, so every row is reachable.',
      '1.1.2: a league you mark Complete or Partial leaves the league overlay and the OBS page, even when MyMTGO missed some of its matches.',
    ],
  },
]
</script>

<template>
  <header class="release-hero">
    <h1 class="release-title">MyMTGO 1.1</h1>
    <p class="release-lede">Archetype detection that works offline and on as many matches as you like.</p>
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
