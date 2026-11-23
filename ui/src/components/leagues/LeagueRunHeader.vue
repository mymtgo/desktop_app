<script setup lang="ts">
import { Calendar, CircleX, Clock, Moon, Sun, Sunrise, Sunset, TriangleAlert, Wrench } from 'lucide-vue-next'
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import type { TimeOfDayView } from '@/bindings'
import LeagueResultBadge from '@/components/leagues/LeagueResultBadge.vue'
import ResultDots from '@/components/leagues/ResultDots.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Badge } from '@/components/ui/badge'
import { lastPlayedText } from '@/lib/decks'
import type { LeagueRunHeaderView } from '@/lib/leagueRunHeader'
import { avgMinutes, startedDayMonth, timeOfDayLabel, tixText, tixTone } from '@/lib/leagues'

/**
 * One league run's header (0.x `LeagueCard`'s), shared by the Leagues page,
 * the deck dashboard and the Limited index: the score, the mana colours,
 * name, format, Manual, version and state badges, the result dots, and the
 * info line (start, average match length, time of day, tix).
 *
 * Slots: `title-extra` after the dots, `info-extra` at the end of the info
 * line, the default slot for lines under the info line, `actions` on the
 * right. A click on the name opens its link and goes no further; the host
 * puts its own click (and hover) on the header through attributes.
 */
const props = withDefaults(
  defineProps<{
    header: LeagueRunHeaderView
    /** "Now" for the relative start (the data's fetch time); the render time when left out. */
    now?: number
  }>(),
  { now: undefined },
)

const startedAt = computed(() => props.header.startedAtMs)
const startedHuman = computed(() =>
  startedAt.value === null ? null : lastPlayedText(startedAt.value, props.now ?? Date.now()),
)
const startedAbsolute = computed(() => (startedAt.value === null ? null : startedDayMonth(startedAt.value)))
const avgMin = computed(() => avgMinutes(props.header.avgMatchSeconds))
const tix = computed(() => tixText(props.header.tixDelta))

const timeOfDayIcons: Record<TimeOfDayView, typeof Sun> = {
  morning: Sunrise,
  afternoon: Sun,
  evening: Sunset,
  night: Moon,
}
const todIcon = computed(() => (props.header.timeOfDay ? timeOfDayIcons[props.header.timeOfDay] : null))
const todLabel = computed(() => timeOfDayLabel(props.header.timeOfDay))

const stateTone = computed(() =>
  props.header.state?.tone === 'destructive'
    ? 'border-destructive/40 bg-destructive/10 text-destructive'
    : 'border-warning/40 bg-warning/10 text-warning',
)
</script>

<template>
  <div class="flex w-full items-center gap-4 px-4 py-3 text-left">
    <LeagueResultBadge
      :classification="header.classification"
      :wins="header.wins"
      :losses="header.losses"
      :live-round="header.liveRound"
    />

    <div class="flex min-w-0 flex-1 flex-col gap-1">
      <div class="flex flex-wrap items-center gap-2 text-sm">
        <template v-if="header.name !== null">
          <ManaColors v-if="header.colors.length" :colors="header.colors" size="xs" class="shrink-0" />
          <RouterLink v-if="header.link" :to="header.link" class="truncate font-medium hover:underline" @click.stop>
            {{ header.name }}
          </RouterLink>
          <span v-else class="truncate font-medium">{{ header.name }}</span>
          <Badge v-if="header.format" variant="outline" class="shrink-0 text-[10px] tracking-wider uppercase">
            {{ header.format }}
          </Badge>
        </template>
        <Badge
          v-if="header.manual"
          variant="outline"
          class="shrink-0 gap-1 border-primary/40 bg-primary/10 text-[10px] tracking-wider text-primary uppercase"
          title="Manually created league"
        >
          <Wrench class="size-3" />
          Manual
        </Badge>
        <span v-if="header.versionLabel" class="text-xs text-muted-foreground">{{ header.versionLabel }}</span>
        <Badge
          v-if="header.state"
          variant="outline"
          class="shrink-0 gap-1 text-[10px] tracking-wider uppercase"
          :class="stateTone"
          :title="header.state.title ?? undefined"
        >
          <CircleX v-if="header.state.tone === 'destructive'" class="size-3" />
          <TriangleAlert v-else class="size-3" />
          {{ header.state.label }}
        </Badge>
        <ResultDots :results="header.results" />
        <slot name="title-extra" />
      </div>

      <div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
        <span v-if="startedHuman" class="inline-flex items-center gap-1">
          <Calendar class="size-3" />
          {{ startedHuman }}
          <span> · {{ startedAbsolute }}</span>
        </span>
        <span v-if="avgMin" class="inline-flex items-center gap-1">
          <Clock class="size-3" /> avg
          <span class="font-medium text-foreground">{{ avgMin }}m</span>
        </span>
        <span v-if="todLabel" class="inline-flex items-center gap-1">
          <component :is="todIcon" class="size-3" /> {{ todLabel }}
        </span>
        <span v-if="tix" class="inline-flex items-center gap-1 tabular-nums" :class="tixTone(header.tixDelta)">
          {{ tix }}
        </span>
        <slot name="info-extra" />
      </div>

      <slot />
    </div>

    <div class="flex shrink-0 items-center gap-1">
      <slot name="actions" />
    </div>
  </div>
</template>
