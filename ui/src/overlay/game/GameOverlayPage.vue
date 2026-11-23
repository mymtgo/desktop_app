<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { commands, events, type GameOverlayView, type OverlayArchetypeChoice, type OverlayArchetypeOption } from '@/bindings'
import { useTauriEvents } from '@/composables/useTauriEvents'
import { errorMessage, unwrap } from '@/lib/commands'
import { FIT_DEBOUNCE_MS, NO_SECTIONS, hasTabSections, isNewer, tabAfter } from '@/lib/overlay'
import DrawOddsPanel from './DrawOddsPanel.vue'
import OpponentHeader from './OpponentHeader.vue'
import OverlayTabs from './OverlayTabs.vue'
import RevealedCards from './RevealedCards.vue'
import SideboardGuide from './SideboardGuide.vue'

/**
 * The game overlay window (overlay spec 7; 0.x `GameOverlay.vue`). The
 * first view comes from `game_overlay`, then `game-overlay-changed` pushes
 * with a newer revision replace it (nothing polls). The page measures its
 * fixed region and tab bar and asks the overlay worker to fit the window
 * around them (debounced 100 ms, sent again only on a changed measure, or
 * forced by a new section set), following the header with a
 * `ResizeObserver` while there is no tab area or the overlay is collapsed.
 */
const view = ref<GameOverlayView | null>(null)
let revision: number | null = null
const activeTab = ref('draw-odds')
const collapsed = ref(false)
const options = ref<OverlayArchetypeOption[] | null>(null)

function show(next: GameOverlayView, nextRevision: number): void {
  if (!isNewer(nextRevision, revision)) return
  const wasSideboarding = view.value?.isSideboarding ?? false
  revision = nextRevision
  view.value = next
  collapsed.value = next.collapsed
  activeTab.value = tabAfter(activeTab.value, wasSideboarding, next)
}

const tabs = computed(() => (view.value ? hasTabSections(view.value.sections) : false))

// ---- Fit and collapse ----

const root = ref<HTMLElement | null>(null)
const fixedRegion = ref<HTMLElement | null>(null)
let lastSentFixedHeight: number | null = null
let fitTimer: ReturnType<typeof setTimeout> | null = null

function measureFixed(): number {
  return Math.round(fixedRegion.value?.offsetHeight ?? 0)
}

/** The tab bar, which stays when the overlay is collapsed. */
function measureBar(): number {
  return Math.round(root.value?.querySelector<HTMLElement>('[data-overlay-bar]')?.offsetHeight ?? 0)
}

async function fitWindow(): Promise<void> {
  const measured = measureFixed()
  if (measured === lastSentFixedHeight) return
  lastSentFixedHeight = measured
  try {
    unwrap(await commands.fitGameOverlay(measured, measureBar()))
  } catch {
    // The next measure tries this height again.
    lastSentFixedHeight = null
  }
}

function scheduleFit(): void {
  if (fitTimer) clearTimeout(fitTimer)
  fitTimer = setTimeout(() => void fitWindow(), FIT_DEBOUNCE_MS)
}

async function setCollapsed(next: boolean): Promise<void> {
  collapsed.value = next
  await nextTick()
  const fixed = measureFixed()
  lastSentFixedHeight = fixed
  try {
    unwrap(await commands.setGameOverlayCollapsed(next, fixed, measureBar()))
  } catch {
    // The page keeps the choice; the next fit retries the window size.
    lastSentFixedHeight = null
  }
}

const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(scheduleFit)
// A collapsed overlay has no scrolling area to absorb a taller header.
const followsHeader = computed(() => !tabs.value || collapsed.value)

watch(
  followsHeader,
  (follows) => {
    observer?.disconnect()
    if (follows && fixedRegion.value) observer?.observe(fixedRegion.value)
  },
  { flush: 'post' },
)

watch(
  () => JSON.stringify(view.value?.sections ?? null),
  () => {
    // A new section set changes the target height even when the fixed
    // region measures the same.
    lastSentFixedHeight = null
    scheduleFit()
  },
  { flush: 'post' },
)

// ---- The archetype picker ----

async function loadOptions(): Promise<void> {
  try {
    options.value = unwrap(await commands.overlayArchetypeOptions())
  } catch (failure) {
    console.warn(errorMessage(failure))
    options.value = []
  }
}

watch(
  () => view.value?.format ?? null,
  () => void loadOptions(),
)

async function select(option: OverlayArchetypeOption): Promise<void> {
  const choice: OverlayArchetypeChoice =
    option.id !== null
      ? { kind: 'held', id: option.id }
      : { kind: 'unheld', uuid: option.uuid, name: option.name, colors: option.colors.join('') || null }
  try {
    unwrap(await commands.setOverlayOpponentArchetype(choice))
  } catch (failure) {
    console.warn(errorMessage(failure))
  }
}

// ---- Life cycle ----

const listening = useTauriEvents(() => [
  events.gameOverlayChanged.listen((event) => show(event.payload.view, event.payload.revision)),
  // The metagame list arrived: the picker can offer unmet archetypes.
  events.metagameSharesReady.listen(() => void loadOptions()),
])

onMounted(async () => {
  if (followsHeader.value && fixedRegion.value) observer?.observe(fixedRegion.value)
  if (!(await listening.ready)) return
  const first = unwrap(await commands.gameOverlay())
  // A push that arrived meanwhile has a newer revision and stays.
  show(first.view, first.revision)
  void loadOptions()
  scheduleFit()
})

onBeforeUnmount(() => {
  observer?.disconnect()
  if (fitTimer) clearTimeout(fitTimer)
})
</script>

<template>
  <div ref="root" class="flex h-screen flex-col bg-background text-foreground select-none">
    <template v-if="view">
      <div ref="fixedRegion" class="shrink-0">
        <OpponentHeader
          v-if="view.sections.opponent"
          :opponent="view.opponent"
          :game="view.game"
          :options="options"
          @select="select"
          @open="loadOptions"
        />
        <p v-if="!tabs" data-tauri-drag-region class="p-3 text-center text-xs text-muted-foreground">{{ NO_SECTIONS }}</p>
      </div>

      <OverlayTabs
        v-if="tabs"
        v-model="activeTab"
        :sections="view.sections"
        :collapsed="collapsed"
        @update:collapsed="setCollapsed"
      >
        <template #draw-odds>
          <DrawOddsPanel :draw-odds="view.drawOdds" />
        </template>
        <template #reveals>
          <RevealedCards :reveals="view.reveals" :potential="view.potential" :has-match="view.hasMatch" />
        </template>
        <template #sideboard>
          <SideboardGuide
            :sideboard="view.sideboard"
            :notes="view.notes"
            :has-match="view.hasMatch"
            :has-deck="view.hasDeck"
            :has-archetype="view.hasArchetype"
          />
        </template>
      </OverlayTabs>
    </template>
  </div>
</template>
