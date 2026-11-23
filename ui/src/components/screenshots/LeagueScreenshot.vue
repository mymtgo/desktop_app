<script setup lang="ts">
import { computed } from 'vue'
import type { RunCardView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import { NONE, durationText, leagueScoreText, playSideText, resultText, threeGames } from '@/lib/screenshots'

/**
 * "Copy screenshot of league" (0.x `LeagueScreenshot`): the deck (a limited
 * run's name), format and score, then a row per match with the opponent,
 * their archetype, games 1 to 3 with the play side, and the match time. The cover art, when given,
 * is a faint background (a `data:` URL, so the capture is complete).
 */
const props = defineProps<{
  league: RunCardView
  coverArtBase64?: string | null
}>()

const colors = {
  bg: '#111111',
  text: '#ffffff',
  muted: 'rgba(255, 255, 255, 0.5)',
  win: '#22c55e',
  loss: '#ef4444',
  border: 'rgba(255, 255, 255, 0.1)',
}

const score = computed(() => leagueScoreText(props.league.results))
/** A limited run has no deck: its name ("QNW Draft") and its deck's colours. */
const title = computed(() => (props.league.limited ? props.league.name : (props.league.deck?.name ?? 'Unknown Deck')))
const manaColors = computed(() => (props.league.limited ? (props.league.colors ?? []) : (props.league.deck?.colors ?? [])))
const rows = computed(() =>
  props.league.matches.map((match) => ({ match, games: threeGames(match.games) })),
)

const narrow = { flexShrink: '0' }
</script>

<template>
  <div
    :style="{
      width: '720px',
      backgroundColor: colors.bg,
      color: colors.text,
      fontFamily: 'system-ui, -apple-system, sans-serif',
      padding: '20px 24px',
      borderRadius: '12px',
      position: 'relative',
      overflow: 'hidden',
    }"
  >
    <img
      v-if="coverArtBase64"
      :src="coverArtBase64"
      alt=""
      :style="{
        position: 'absolute',
        top: '0',
        left: '0',
        width: '100%',
        height: '100%',
        objectFit: 'cover',
        objectPosition: 'top',
        opacity: '0.25',
        pointerEvents: 'none',
      }"
    />
    <div :style="{ position: 'relative' }">
      <div :style="{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start' }">
        <div>
          <div :style="{ display: 'flex', alignItems: 'center', gap: '8px' }">
            <span :style="{ fontSize: '16px', fontWeight: '700', lineHeight: '1.2' }">
              {{ title }}
            </span>
            <ManaColors v-if="manaColors.length" :colors="manaColors" size="xs" />
          </div>
          <div :style="{ fontSize: '12px', color: colors.muted, marginTop: '2px' }">{{ league.format }}</div>
        </div>
        <div :style="{ fontSize: '16px', fontWeight: '700', fontVariantNumeric: 'tabular-nums', paddingTop: '2px' }">
          {{ score }}
        </div>
      </div>

      <div :style="{ marginTop: '14px', fontSize: '12px' }">
        <div
          :style="{
            display: 'flex',
            alignItems: 'center',
            padding: '6px 8px',
            color: colors.muted,
            fontSize: '10px',
            textTransform: 'uppercase',
            letterSpacing: '0.05em',
          }"
        >
          <div :style="{ ...narrow, width: '24px' }">#</div>
          <div :style="{ ...narrow, width: '50px' }">Result</div>
          <div :style="{ ...narrow, width: '100px' }">Opponent</div>
          <div :style="{ flex: '1' }">Vs</div>
          <div :style="{ ...narrow, width: '80px', textAlign: 'center' }">Game 1</div>
          <div :style="{ ...narrow, width: '80px', textAlign: 'center' }">Game 2</div>
          <div :style="{ ...narrow, width: '80px', textAlign: 'center' }">Game 3</div>
          <div :style="{ ...narrow, width: '40px', textAlign: 'right' }">Time</div>
        </div>
        <div
          v-for="(row, index) in rows"
          :key="row.match.token"
          :style="{
            display: 'flex',
            alignItems: 'center',
            padding: '6px 8px',
            borderRadius: '4px',
            backgroundColor: index % 2 === 0 ? 'rgba(255, 255, 255, 0.04)' : 'transparent',
          }"
        >
          <div
            :style="{ ...narrow, color: colors.muted, fontWeight: '500', width: '24px', fontVariantNumeric: 'tabular-nums' }"
          >
            {{ index + 1 }}
          </div>
          <div
            :style="{ ...narrow, color: row.match.result === 'W' ? colors.win : colors.loss, fontWeight: '600', width: '50px' }"
          >
            {{ resultText(row.match.result) }}
          </div>
          <div :style="{ ...narrow, width: '100px', fontWeight: '500' }">{{ row.match.opponentName ?? NONE }}</div>
          <div
            :style="{ flex: '1', color: row.match.opponentArchetype ? colors.text : colors.muted, paddingRight: '8px' }"
          >
            {{ row.match.opponentArchetype ?? 'Unknown' }}
          </div>
          <div v-for="(game, g) in row.games" :key="g" :style="{ ...narrow, width: '80px', textAlign: 'center' }">
            <template v-if="game">
              <span :style="{ color: game.result === 'W' ? colors.win : colors.loss, fontWeight: '600' }">
                {{ resultText(game.result) }}
              </span>
              <span v-if="playSideText(game)" :style="{ color: colors.muted, marginLeft: '4px', fontSize: '10px' }">
                {{ playSideText(game) }}
              </span>
            </template>
            <span v-else :style="{ color: colors.muted }">{{ NONE }}</span>
          </div>
          <div
            :style="{ ...narrow, width: '40px', textAlign: 'right', color: colors.muted, fontVariantNumeric: 'tabular-nums' }"
          >
            {{ durationText(row.match.durationSeconds) }}
          </div>
        </div>
      </div>

      <div :style="{ marginTop: '12px', textAlign: 'right', fontSize: '10px', color: colors.muted }">mymtgo.com</div>
    </div>
  </div>
</template>
