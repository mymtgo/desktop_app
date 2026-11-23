<script setup lang="ts">
import { LayoutDashboard, PencilLine, Shapes, Swords } from 'lucide-vue-next'
import { RouterLink } from 'vue-router'
import icon from './1.0-icon.png'

// What's new in 1.0 (release spec section 7, ruling 11), text approved by
// the owner on 2026-10-19. Shown
// by itself to players moving from 0.x, so it opens as the launch
// announcement: the ledger icon as the hero, then what is new since 0.x
// (from the beta release notes, beta.5 to beta.8), then the practical notes.
// The 1.0 patch releases' fixes, newest first, come straight after the hero,
// so a player following the link after a patch sees them without scrolling.

// Data, not markup: a long static list compiles to one static block, which
// the test renderer cannot mount.
const newSince0x = [
  {
    name: 'Dashboard',
    icon: LayoutDashboard,
    text: 'A new home page. Pick Constructed, Limited or a single format, and see your results, league finishes, top matchups, recent decks and your last session. Your drafts count too.',
  },
  {
    name: 'Manage your matches and leagues',
    icon: PencilLine,
    text: "Fix a match's opponent, times and games, move matches between leagues, set a run's status, and undo or restore anything you delete.",
  },
  {
    name: 'Versus overlay',
    icon: Swords,
    text: "A fighting-game theme for the league card with live life bars, and a line on the game overlay showing who is on the play and each player's kept hand size.",
  },
  {
    name: 'Metagame and archetypes',
    icon: Shapes,
    text: 'Archetype tiles show key cards, your share against the global share and your record against them.',
  },
]

const smallerPoints = [
  {
    name: 'Faster and lighter',
    text: 'rebuilt on a new stack and designed to be more efficient, so it stays out of the way while MTGO runs.',
  },
  {
    name: 'A sturdier foundation',
    text: "matches are rebuilt from MTGO's own log, so when MyMTGO learns to read a log better, your past matches can be corrected too.",
  },
  {
    name: 'Updates arrive quicker',
    text: 'MyMTGO looks for a new version every hour, downloads it in the background and installs it when you choose.',
  },
  {
    name: 'Linux',
    text: 'MyMTGO now runs on Linux too, for MTGO played under Wine or Proton, with the tray icon and launch at login.',
  },
]

const patches = [
  {
    version: '1.0.4',
    fixes: [
      'Overlay: minimise an overlay window by right-clicking it, or from the taskbar.',
      "Overlay: when MyMTGO can't be reached, the game overlay says so instead of showing an unknown archetype.",
      "Overlay: the opponent's archetype estimate updates sooner as their cards are revealed.",
      'Matches: a match whose archetypes have not been detected yet says "Not detected yet".',
      'Stream overlay: the setup card reminds you to tick "Refresh browser when scene becomes active" in OBS.',
      'MyMTGO server: large uploads no longer fail, MyMTGO slows down when the server asks it to, and it sends fewer requests that cannot succeed.',
      'Security: each window can only use the app features it needs.',
    ],
  },
  {
    version: '1.0.3',
    fixes: [
      'Leagues: each league run now has the same id on every device, so runs played on two devices are no longer merged into one in League finishes on mymtgo.com.',
    ],
  },
  {
    version: '1.0.2',
    fixes: [
      'Limited: Vintage Cube, QuickCube and other cube or mixed-set drafts are named by their format, not by a random set from their picks.',
      'Archetypes: when an estimate fails, the opponent is left unlabelled and tried again later, instead of getting a wrong guess such as Ruby Storm shown as Izzet Aggro.',
      "Archetypes: custom archetypes (including ones imported from 0.x) label only your own decks, so they no longer override MyMTGO's label for opponents.",
    ],
  },
  {
    version: '1.0.1',
    fixes: [
      "Archetypes: the archetype picker lists every archetype for the format, including ones you haven't played against yet.",
      "Archetypes: detection asks MyMTGO first when online, so a deck is no longer labelled as a similar archetype just because that's the only one you've met.",
      "Windows: updating creates the MyMTGO shortcuts even if MyMTGO 0.x's shortcuts are still there.",
    ],
  },
]
</script>

<template>
  <header class="launch-hero">
    <img :src="icon" alt="" class="launch-book" />
    <h1 class="launch-title">MyMTGO 1.0</h1>
    <p class="launch-lede">
      A new MyMTGO, rebuilt from the ground up. It still reads MTGO's own log and deck files while you play, keeps the
      pages you know from 0.x, and adds a few you will want.
    </p>
  </header>

  <h2>Fixes since 1.0.0</h2>

  <section v-for="patch in patches" :key="patch.version" class="launch-patch">
    <h3>{{ patch.version }}</h3>
    <ul>
      <li v-for="fix in patch.fixes" :key="fix">{{ fix }}</li>
    </ul>
  </section>

  <h2>New since MyMTGO 0.x</h2>

  <ul class="launch-features">
    <li v-for="feature in newSince0x" :key="feature.name" class="launch-feature">
      <component :is="feature.icon" :size="18" class="launch-feature-icon" aria-hidden="true" />
      <div>
        <h3 class="launch-feature-name">{{ feature.name }}</h3>
        <p>{{ feature.text }}</p>
      </div>
    </li>
  </ul>

  <ul class="launch-points">
    <li v-for="point in smallerPoints" :key="point.name">
      <strong>{{ point.name }}:</strong> {{ point.text }}
    </li>
  </ul>

  <h2>Coming from MyMTGO 0.x</h2>

  <p>
    Your matches, decks, leagues, drafts and settings from MyMTGO 0.x can come with you. If you chose to migrate when
    you installed, MyMTGO has imported them already. If not, open the
    <RouterLink to="/import">Import</RouterLink> page and use the <strong>MyMTGO 0.x</strong> section at any time.
  </p>

  <p>
    MyMTGO 0.x's data stays on this computer, so you can import again later. You can uninstall MyMTGO 0.x once the
    import is done.
  </p>

  <h2>Update channels</h2>

  <p>
    MyMTGO updates itself from the stable channel. To get new versions early, turn on
    <strong>Enable edge updates</strong> in Settings, General. Edge builds can have rough edges.
  </p>
</template>

<style scoped>
/* The ledger's parchment, from the app icon: the one warm colour on the page.
   No glow or shadow on the icon: the Windows webview drew them as a box. */
.launch-hero {
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.75rem;
  padding: 2.5rem 0 1.5rem;
  text-align: center;
}

.launch-book {
  position: relative;
  width: 7rem;
  height: 7rem;
  border: 0;
  border-radius: 0;
  animation: launch-open 700ms cubic-bezier(0.2, 0.8, 0.2, 1) both;
}

.launch-title {
  position: relative;
  margin-top: 0.5rem;
  font-family: 'Bungee', sans-serif;
  font-size: clamp(2.25rem, 6vw, 3.5rem);
  font-weight: 400;
  line-height: 1;
  letter-spacing: 0.02em;
  color: var(--color-foreground);
  text-shadow: 3px 3px 0 rgb(0 0 0 / 0.6);
}

.launch-lede {
  position: relative;
  max-width: 34rem;
  font-size: 0.95rem;
  line-height: 1.6;
}

.launch-features {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr));
  gap: 0 2rem;
  padding: 0;
  list-style: none;
}

.launch-feature {
  display: flex;
  gap: 0.75rem;
  padding: 0.9rem 0;
  border-top: 1px solid var(--color-border);
}

.launch-feature-icon {
  flex-shrink: 0;
  margin-top: 0.15rem;
  color: #d9b77e;
}

.launch-feature-name {
  padding-top: 0;
  margin-bottom: 0.2rem;
  font-size: 0.875rem;
}

.launch-points {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr));
  gap: 0.5rem 2rem;
  padding: 0.9rem 0 0;
  border-top: 1px solid var(--color-border);
  font-size: 0.8125rem;
  list-style: none;
}

.launch-patch {
  font-size: 0.8125rem;
}

@keyframes launch-open {
  from {
    opacity: 0;
    transform: translateY(8px) scale(0.92) rotate(-4deg);
  }
  to {
    opacity: 1;
    transform: none;
  }
}

@media (prefers-reduced-motion: reduce) {
  .launch-book {
    animation: none;
  }
}
</style>
