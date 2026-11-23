<script setup lang="ts">
import { ArrowLeftRight, Trophy } from 'lucide-vue-next'
import { computed } from 'vue'
import type { LeagueOverlayState } from '@/bindings'
import { useOverlayDeckLabel } from '@/composables/useOverlayDeckLabel'
import {
  cardChip,
  cardRows,
  compactLeads,
  compactSubline,
  compactTitle,
  displayOf,
  eventLabelCandidates,
  fullSubline,
  labelDeckId,
  recordText,
  showsGames,
  showsMark,
  tournamentCard,
} from '@/lib/leagueOverlayCard'
import FitText from './FitText.vue'
import OverlayGamePips from './OverlayGamePips.vue'
import OverlayLabelInput from './OverlayLabelInput.vue'
import OverlayStatusChip from './OverlayStatusChip.vue'

/**
 * The league card (overlay spec 9.2; 0.x `LeagueOverlayCard.vue` at the
 * `release_0.46.0` tip): an opaque card filling its window, full and
 * compact. `state.display` decides which parts show (league card options
 * spec); the size never changes. The art sits behind the text, or with
 * `art.thumbnail` in a square on the left. A league card's full title is an
 * inline input for the deck's label (`OverlayLabelInput`); a tournament
 * card's title says where the player is (event overlay spec 5.2) and is not
 * editable, its archetype moving to the status line. This is the Classic
 * theme; `LeagueOverlayCard.vue` picks the theme.
 */
const props = defineProps<{ state: LeagueOverlayState; now: number; readonly?: boolean }>()

const display = computed(() => displayOf(props.state))
const chip = computed(() => cardChip(props.state, props.now))
const events = computed(() => eventLabelCandidates(props.state))
const record = computed(() => recordText(props.state))
const rows = computed(() => cardRows(props.state, props.now))
const games = computed(() => showsGames(props.state))
const sideboarding = computed(() => props.state.status === 'sideboarding')
/** The line's candidates for FitText, longest first: a tournament line drops parts from the end. */
function ladder(parts: string[]): string[][] {
  const out: string[][] = []
  for (let count = parts.length; count >= 1; count--) out.push(parts.slice(0, count))
  return out
}
const leads = computed(() => compactLeads(props.state))
const thumbnail = computed(() => (props.state.art?.thumbnail ? props.state.art.url : null))
const background = computed(() => (props.state.art && !props.state.art.thumbnail ? props.state.art.url : null))

const labelDeck = computed(() => labelDeckId(props.state))
// The compact title shows the label; the full card's input (OverlayLabelInput) keeps its own copy.
const { label: customLabel } = useOverlayDeckLabel(
  labelDeck,
  computed(() => props.state.deck?.label ?? null),
)

const tournament = computed(() => tournamentCard(props.state, props.now, customLabel.value))
const fullLines = computed(() => {
  if (tournament.value) return ladder(tournament.value.line)
  const line = fullSubline(props.state, props.now)
  return line ? [[line]] : []
})
const compactLines = computed(() => {
  if (tournament.value) return ladder(tournament.value.compactLine)
  const line = compactSubline(props.state, props.now)
  return line ? [[line]] : []
})
// "Game 2 next" (the first part while sideboarding) is amber.
const amberFirst = computed(() => sideboarding.value && display.value.status)
</script>

<template>
  <!--
    Spacing per event overlay spec 10: a 2 px frame, fixed rows on a 4 px grid, the record on the status line
    baseline. Grid baseline alignment moves the item whose baseline sits higher in its box down to the other's;
    the record's -12 px top margin makes its baseline the higher one, so the record moves and the status line
    keeps its row (without it the status line drops 9 px).
  -->
  <div
    class="h-full w-full p-0.5 font-card text-white"
    :class="state.size === 'compact' ? 'rounded-[10px]' : 'rounded-[14px]'"
  >
    <div
      class="relative flex h-full overflow-hidden bg-[#0c1014] [text-shadow:0_1px_2px_rgb(0_0_0/.8)]"
      :class="state.size === 'compact' ? 'rounded-[8px]' : 'rounded-[12px]'"
    >
      <template v-if="background">
        <div
          data-art="background"
          class="absolute inset-0 bg-cover bg-[center_35%]"
          :style="{ backgroundImage: `url(${JSON.stringify(background)})` }"
        />
        <!-- Darkens the art behind the text, so the text needs only a light shadow. -->
        <div class="absolute inset-0 bg-linear-to-r from-black/80 via-black/55 to-black/25" />
      </template>
      <div v-else-if="!thumbnail" class="absolute inset-0 bg-[radial-gradient(120%_140%_at_85%_0%,#1b2430,#0c1014_60%)]" />

      <div
        v-if="thumbnail"
        data-art="thumbnail"
        class="relative aspect-square h-full shrink-0 bg-cover bg-center"
        :style="{ backgroundImage: `url(${JSON.stringify(thumbnail)})` }"
      />

      <!-- Full: pill row 16, gap 4, title 20, gap 4, status line 12, inside a 10 px inset. -->
      <div
        v-if="state.size === 'full'"
        class="relative flex h-full min-w-0 flex-1 flex-col px-[18px] py-2.5"
        :class="rows.top && rows.bottom ? '' : 'justify-center'"
      >
        <div v-if="rows.top" class="flex h-4 shrink-0 items-center justify-between gap-2">
          <OverlayStatusChip v-if="events.length > 0" tone="neutral" class="shrink">
            <!-- The first label that fits beside the right chip, which never shrinks (spec 5.2). -->
            <FitText v-slot="{ index }" class="truncate" :count="events.length" :fit-key="events.join('|')">{{ events[index] }}</FitText>
          </OverlayStatusChip>
          <span v-else />
          <OverlayStatusChip v-if="chip" :tone="chip.tone" class="shrink-0">
            <ArrowLeftRight v-if="chip.icon === 'swap'" class="size-3" />
            <Trophy v-if="chip.icon === 'trophy'" class="size-3 text-sky-300" />
            {{ chip.label }}
          </OverlayStatusChip>
        </div>
        <div
          v-if="rows.bottom"
          class="grid shrink-0 grid-cols-[minmax(0,1fr)_auto] grid-rows-[20px_12px] gap-x-2 gap-y-1"
          :class="rows.top ? 'mt-1' : ''"
        >
          <div
            v-if="tournament?.title"
            class="col-start-1 row-start-1 flex h-5 min-w-0 items-center gap-1 text-base leading-5 font-bold"
            data-title="tournament"
          >
            <Trophy v-if="tournament.title.icon === 'trophy'" class="size-4 shrink-0 text-sky-300" />
            <span class="truncate">{{ tournament.title.text }}</span>
          </div>
          <!-- The input shows the plain title itself when there is no deck to label. -->
          <OverlayLabelInput
            v-else-if="display.title"
            :state="state"
            :readonly="readonly"
            class="col-start-1 row-start-1 block h-5 w-full min-w-0 text-base leading-5 font-bold"
          />
          <div
            class="col-start-1 row-start-2 flex h-3 min-w-0 items-center gap-2 self-baseline text-[11px] leading-3 text-white/70"
          >
            <!-- A zero-width text node gives the line a baseline when it shows only pips. -->
            <span class="-mr-2 w-0 overflow-hidden">&#8203;</span>
            <OverlayGamePips
              v-if="games"
              :games="state.match?.games ?? []"
              :live="state.status === 'in_game'"
              :mode="display.games"
            />
            <FitText
              v-if="fullLines.length > 0"
              v-slot="{ index }"
              class="min-w-0 truncate"
              :count="fullLines.length"
              :fit-key="fullLines[0].join('|')"
            >
              <template v-for="(part, i) in fullLines[index]" :key="i"
                ><template v-if="i > 0"> · </template
                ><span :class="i === 0 && amberFirst ? 'font-semibold text-amber-300' : ''">{{ part }}</span></template
              >
            </FitText>
          </div>
          <span v-if="record" class="col-start-2 row-start-2 -mt-3 -mr-0.5 self-baseline text-[36px] leading-3 font-extrabold whitespace-nowrap">{{ record }}</span>
        </div>
        <span
          v-if="showsMark(state)"
          class="pointer-events-none absolute right-[18px] bottom-1.5 text-[8px] leading-2 tracking-[.16em] uppercase opacity-45"
        >
          mymtgo.com
        </span>
      </div>

      <!-- Compact: title 16, gap 4, status line 12, centred in the 54 px inside the frame. -->
      <div v-else class="relative flex h-full min-w-0 flex-1 items-center px-3.5">
        <div class="grid w-full grid-cols-[minmax(0,1fr)_auto] grid-rows-[16px_12px] gap-x-3 gap-y-1">
          <div
            v-if="tournament?.title || display.title"
            class="col-start-1 row-start-1 h-4 truncate text-[13px] leading-4 font-bold"
          >
            {{ tournament?.title?.text ?? compactTitle(state, customLabel) }}
          </div>
          <div
            class="col-start-1 row-start-2 flex h-3 min-w-0 items-center gap-2 self-baseline text-[11px] leading-3 text-white/70"
          >
            <span class="-mr-2 w-0 overflow-hidden">&#8203;</span>
            <FitText v-if="leads.length > 0" v-slot="{ index }" class="min-w-0 truncate" :count="leads.length" :fit-key="leads.join('|')">{{ leads[index] }}</FitText>
            <OverlayGamePips
              v-if="games"
              :games="state.match?.games ?? []"
              :live="state.status === 'in_game'"
              :mode="display.games"
            />
            <FitText
              v-if="compactLines.length > 0"
              v-slot="{ index }"
              class="min-w-0 truncate"
              :count="compactLines.length"
              :fit-key="compactLines[0].join('|')"
            >
              <template v-for="(part, i) in compactLines[index]" :key="i"
                ><template v-if="i > 0"> · </template
                ><span :class="i === 0 && amberFirst ? 'font-semibold text-amber-300' : ''">{{ part }}</span></template
              >
            </FitText>
          </div>
          <span v-if="record" class="col-start-2 row-start-2 -mt-3 -mr-px self-baseline text-2xl leading-3 font-extrabold whitespace-nowrap">{{ record }}</span>
        </div>
      </div>
    </div>
  </div>
</template>
