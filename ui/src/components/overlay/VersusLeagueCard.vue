<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import type { LeagueOverlayState } from '@/bindings'
import { useOverlayDeckLabel } from '@/composables/useOverlayDeckLabel'
import { labelDeckId } from '@/lib/leagueOverlayCard'
import FitText from './FitText.vue'
import {
  ANIMATION_MS,
  bars as barsOf,
  bottomLines,
  callout as calloutOf,
  compactMiddle,
  damageChunks,
  introText,
  isLow,
  matchGold,
  matchText,
  nameFit,
  opponentName,
  roundPips,
  sideboardBanner,
  subtitleLadder,
  nextLastMatch,
  versusAnimations,
  yourName,
  type DamageChunk,
  type LastMatch,
  type Side,
  type VersusAnimation,
} from '@/lib/versusCard'

/**
 * The Versus league card (league overlay themes spec 6): a fighting game
 * versus screen. Life bars follow `match.life`; a state change plays the
 * animations `versusAnimations` picks, one after another. The first state
 * plays nothing, so a reload or an OBS refresh never replays one.
 */
const props = defineProps<{ state: LeagueOverlayState; now: number }>()

const compact = computed(() => props.state.size === 'compact')
const lastStart = ref<{ you: number; opponent: number } | null>(null)
const bars = computed(() => barsOf(props.state, lastStart.value))
const pips = computed(() => roundPips(props.state))
const banner = computed(() => sideboardBanner(props.state))
const match = computed(() => matchText(props.state, compact.value))
const middle = computed(() => compactMiddle(props.state))
const lines = computed(() => bottomLines(props.state))
// The deck's label, as the Classic compact title reads it; the full card's
// input (OverlayLabelInput, through the title slot) keeps its own copy.
const { label: customLabel } = useOverlayDeckLabel(
  computed(() => labelDeckId(props.state)),
  computed(() => props.state.deck?.label ?? null),
)
const name = computed(() => yourName(props.state, customLabel.value))
const opponent = computed(() => opponentName(props.state))
const callout = computed(() => calloutOf(props.state, props.now, customLabel.value))
const subtitles = computed(() => (callout.value ? subtitleLadder(callout.value) : []))
const gold = computed(() => matchGold(props.state))
// Long names step down in size to fit their slot (CHALLENGER always fits).
const yourFit = computed(() => `fit-${nameFit(name.value)}`)
const opponentFit = computed(() => `fit-${nameFit(opponent.value)}`)
const showsFight = computed(() => props.state.status === 'in_game' || props.state.status === 'sideboarding')

// The damage chunks: the share each bar just lost, fading out. The key
// restarts the fade when the same side drops again.
const ghosts = ref<Record<Side, (DamageChunk & { key: number }) | null>>({ you: null, opponent: null })
let ghostKey = 0

// The animation playing now and the ones waiting.
const playing = ref<VersusAnimation | null>(null)
const playKey = ref(0)
let queue: VersusAnimation[] = []
// The side that lost the last K.O.: it stays at zero until the next game starts.
const knockedOut = ref<Side | null>(null)
// The match last seen, carried between pushes (seeded from the first state).
let lastMatch: LastMatch = nextLastMatch(props.state, null, [])
let timer: ReturnType<typeof setTimeout> | null = null

function playNext(): void {
  timer = null
  const next = queue.shift() ?? null
  playing.value = next
  if (next) {
    if (next.kind === 'ko') knockedOut.value = next.loser
    if (next.kind === 'fight') knockedOut.value = null
    playKey.value += 1
    timer = setTimeout(playNext, ANIMATION_MS[next.kind])
  }
}

watch(
  () => props.state,
  (next, prev) => {
    if (next.status !== 'in_game') knockedOut.value = null
    const life = next.status === 'in_game' ? next.match?.life : null
    if (life) lastStart.value = { you: life.startYou, opponent: life.startOpponent }
    const before = prev ? barsOf(prev, lastStart.value) : null
    const after = barsOf(next, lastStart.value)
    for (const chunk of damageChunks(before, after)) {
      ghostKey += 1
      ghosts.value[chunk.side] = { ...chunk, key: ghostKey }
    }
    const animations = versusAnimations(prev ?? null, next, lastMatch)
    lastMatch = nextLastMatch(next, lastMatch, animations)
    if (animations.length === 0) return
    queue.push(...animations)
    if (!timer) playNext()
  },
)

onBeforeUnmount(() => {
  if (timer) clearTimeout(timer)
})

/** During a K.O. the loser's bar and number show 0, whatever the life was (a concession). */
function share(side: Side): number {
  const now = playing.value
  if (now?.kind === 'ko' && now.loser === side) return 0
  if (knockedOut.value === side) return 0
  return bars.value[side]
}

function lifeText(side: Side): string | null {
  const now = playing.value
  if (now?.kind === 'ko' && now.loser === side) return '0'
  if (knockedOut.value === side) return '0'
  const life = side === 'you' ? bars.value.youLife : bars.value.opponentLife
  return life === null ? null : String(life)
}

function low(side: Side): boolean {
  const life = side === 'you' ? bars.value.youLife : bars.value.opponentLife
  return props.state.status === 'in_game' && life !== null && isLow(life)
}
</script>

<template>
  <div class="versus" :class="compact ? 'versus-compact' : 'versus-full'" data-theme="versus">
    <!-- The bars and VS stay behind a callout, dimmed by its backdrop (event overlay spec 5.4 callouts). -->
    <div v-for="side in ['you', 'opponent'] as const" :key="side" class="bar" :class="[side, { low: low(side) }]">
      <div class="hp" :style="{ width: `${share(side) * 100}%` }" />
      <div
        v-if="ghosts[side]"
        :key="ghosts[side].key"
        class="ghost"
        :style="{ width: `${(ghosts[side].from - ghosts[side].to) * 100}%`, [side === 'you' ? 'right' : 'left']: `${ghosts[side].to * 100}%` }"
      />
    </div>
    <div class="vs">
      <span class="vs-mark">VS</span>
      <span v-if="match && !compact && (callout === null || showsFight)" class="vs-match" :class="{ gold }">{{ match }}</span>
    </div>
    <template v-if="callout === null || showsFight">
      <span v-if="compact && lifeText('you') !== null" class="life you">{{ lifeText('you') }}</span>
      <span v-if="compact && lifeText('opponent') !== null" class="life opponent">{{ lifeText('opponent') }}</span>
      <template v-if="compact">
        <!-- Compact pips sit under the bars so the names get the whole bottom row. -->
        <div class="pips you"><i v-for="(on, index) in pips.you" :key="index" class="pip" :class="{ on }" /></div>
        <div class="pips opponent"><i v-for="(on, index) in pips.opponent" :key="index" class="pip" :class="{ on }" /></div>
        <div class="row" data-row="compact">
          <span class="rname you" :class="yourFit"><slot name="title">{{ name }}</slot></span>
          <span class="mid" :class="{ gold: banner }">
            <template v-if="middle?.lead">{{ middle.lead }}</template>
            <template v-if="middle?.lead && middle.record"> · </template>
            <b v-if="middle?.record" class="record">{{ middle.record }}</b>
          </span>
          <span class="rname opponent" :class="opponentFit">{{ opponent }}</span>
        </div>
      </template>
      <template v-else>
        <!-- One baseline row per side: the name at the outer edge, the life number toward VS. -->
        <div class="side you">
          <div class="name you" :class="yourFit"><slot name="title">{{ name }}</slot></div>
          <span v-if="lifeText('you') !== null" class="life you">{{ lifeText('you') }}</span>
        </div>
        <div class="side opponent">
          <div class="name opponent" :class="opponentFit">{{ opponent }}</div>
          <span v-if="lifeText('opponent') !== null" class="life opponent">{{ lifeText('opponent') }}</span>
        </div>
        <div class="pips you"><i v-for="(on, index) in pips.you" :key="index" class="pip" :class="{ on }" /></div>
        <div class="pips opponent"><i v-for="(on, index) in pips.opponent" :key="index" class="pip" :class="{ on }" /></div>
        <div v-if="banner" class="banner">{{ banner }}</div>
        <!-- The longest meta line that fits: the rank drops first, then the label steps down (spec 5.2). -->
        <FitText
          v-else-if="lines.length > 0"
          v-slot="{ index }"
          tag="div"
          class="meta"
          :count="lines.length"
          :fit-key="JSON.stringify(lines[0])"
        >
          <template v-if="lines[index].lead">{{ lines[index].lead }}</template>
          <template v-if="lines[index].lead && lines[index].record"> · </template>
          <b v-if="lines[index].record" class="record">{{ lines[index].record }}</b>
          <template v-if="lines[index].tail && (lines[index].lead || lines[index].record)"> · </template>
          <template v-if="lines[index].tail">{{ lines[index].tail }}</template>
        </FitText>
      </template>
    </template>

    <div v-if="callout && !showsFight" class="callout" :class="[callout.tone, `size-${state.status}`, { pulse: callout.pulse }]">
      <span class="callout-title">{{ callout.title }}</span>
      <!-- The longest subtitle that fits: parts are dropped from the end (spec 5.2). -->
      <FitText
        v-if="!compact && callout.subtitle"
        v-slot="{ index }"
        class="callout-subtitle"
        :count="subtitles.length"
        :fit-key="callout.subtitle"
      >{{ subtitles[index] }}</FitText>
    </div>

    <div v-if="playing" :key="playKey" class="play" :data-animation="playing.kind">
      <template v-if="playing.kind === 'fight'">
        <span class="round-in">{{ introText(state, playing.round) }}</span>
        <span class="fight">FIGHT!</span>
      </template>
      <template v-else-if="playing.kind === 'ko'">
        <i class="flash" />
        <span class="ko">K.O.</span>
      </template>
      <span v-else-if="playing.kind === 'draw'" class="ko draw">DRAW</span>
      <span v-else class="round-big">GAME {{ playing.round }}<small>SIDEBOARDING</small></span>
    </div>
  </div>
</template>

<style scoped>
/* Values from the league overlay themes spec, section 6, on the event overlay spec 10 grid. CSS tops are outer y minus the 2 px border. Every Teko and Bungee text has an explicit line height. */
.versus { position: relative; width: 100%; height: 100%; overflow: hidden; color: #fff; font-family: 'Teko', sans-serif; background: linear-gradient(180deg, rgb(8 10 16 / 0.92), rgb(8 10 16 / 0.78)); border-top: 2px solid #ff3b5c; border-bottom: 2px solid #22d3ee; box-sizing: border-box; }
.bar { position: absolute; overflow: hidden; background: #1d2230; box-shadow: inset 0 0 0 2px rgb(255 255 255 / 0.15); }
.bar.you { transform: skewX(-20deg); }
.bar.opponent { transform: skewX(20deg); }
.hp { position: absolute; top: 0; bottom: 0; transition: width 0.25s steps(4); }
.bar.you .hp { right: 0; background: linear-gradient(90deg, #ff3b5c, #ff9a3c); }
.bar.opponent .hp { left: 0; background: linear-gradient(90deg, #22d3ee, #6c7bff); }
.bar.low .hp { animation: low 0.6s ease-in-out infinite; }
.ghost { position: absolute; top: 0; bottom: 0; background: rgb(255 255 255 / 0.6); animation: drain 1.4s ease-in forwards; }
.life { font-weight: 700; text-shadow: 2px 2px 0 #000; }
.vs { position: absolute; left: 50%; transform: translateX(-50%); display: flex; flex-direction: column; align-items: center; }
.vs-mark { font-family: 'Bungee', sans-serif; background: linear-gradient(#fff, #ffd84a); -webkit-background-clip: text; background-clip: text; color: transparent; filter: drop-shadow(2px 2px 0 #000); }
.vs-match { letter-spacing: 2px; color: #aab3c5; white-space: nowrap; }
.vs-match.gold { color: #ffd84a; }
.side { position: absolute; display: flex; justify-content: space-between; align-items: baseline; }
.side.opponent { flex-direction: row-reverse; }
.name { min-width: 0; font-weight: 700; letter-spacing: 1px; text-transform: uppercase; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.name.opponent { text-align: right; color: #cfd6e4; }
.pips { position: absolute; display: flex; }
.pip { display: block; background: #2a3040; transform: skewX(-20deg); }
.pips.you .pip.on { background: #ff3b5c; box-shadow: 0 0 8px #ff3b5c; }
.pips.opponent .pip.on { background: #22d3ee; box-shadow: 0 0 8px #22d3ee; }
/* The full card width less 16 px each side, centred: a box at left 50% could only be half the card wide. */
.meta { position: absolute; left: 16px; right: 16px; text-align: center; letter-spacing: 2px; color: #aab3c5; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.banner { position: absolute; left: 16px; right: 16px; text-align: center; overflow: hidden; font-family: 'Bungee', sans-serif; letter-spacing: 3px; color: #ffd84a; text-shadow: 0 0 10px rgb(255 216 74 / 0.7), 2px 2px 0 #000; white-space: nowrap; animation: banner 2s ease-in-out infinite; }
.callout { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; text-align: center; background: rgb(8 10 16 / 0.6); }
.callout-title { white-space: nowrap; font-family: 'Bungee', sans-serif; letter-spacing: 3px; text-shadow: 3px 3px 0 #000; }
.callout-subtitle { white-space: nowrap; letter-spacing: 3px; color: #aab3c5; max-width: calc(100% - 32px); overflow: hidden; text-overflow: ellipsis; }
.callout.cyan .callout-title { text-shadow: 0 0 18px #22d3ee, 3px 3px 0 #000; }
.callout.red .callout-title { text-shadow: 0 0 18px #ff3b5c, 3px 3px 0 #000; }
.callout.gold .callout-title { text-shadow: 0 0 22px #ffd84a, 3px 3px 0 #000; }
.callout.grey .callout-title { color: #aab3c5; }
.callout.pulse .callout-title { animation: pulse 1.2s ease-in-out infinite; }
.play { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; background: rgb(8 10 16 / 0.55); pointer-events: none; }
.round-in { position: absolute; font-family: 'Bungee', sans-serif; letter-spacing: 4px; text-shadow: 0 0 16px #22d3ee, 3px 3px 0 #000; animation: round-in 2.2s forwards; }
.fight { position: absolute; font-family: 'Bungee', sans-serif; letter-spacing: 4px; color: #ffd84a; -webkit-text-stroke: 2px #ff3b5c; text-shadow: 0 0 24px #ff3b5c, 4px 4px 0 #000; opacity: 0; animation: fight 2.2s forwards; }
.ko { font-family: 'Bungee', sans-serif; font-size: 52px; line-height: 52px; letter-spacing: 6px; text-shadow: 0 0 18px #ff3b5c, 4px 4px 0 #000; animation: ko 2.5s forwards; }
.flash { position: absolute; inset: 0; background: #ff3b5c; opacity: 0; animation: ko-flash 2.5s forwards; }
.ko.draw { text-shadow: 0 0 18px #fff, 4px 4px 0 #000; }
.play[data-animation='ko'] { animation: shake 0.16s steps(4) 1; }
.round-big { display: flex; flex-direction: column; align-items: center; font-family: 'Bungee', sans-serif; font-size: 44px; line-height: 44px; letter-spacing: 4px; text-shadow: 0 0 18px #ffd84a, 3px 3px 0 #000; animation: round-big 2.4s forwards; }
.round-big small { font-family: 'Teko', sans-serif; font-size: 18px; line-height: 20px; letter-spacing: 3px; color: #aab3c5; margin-top: 4px; }
.record { color: #fff; font-weight: 700; }

/* Full: 580 x 104. Bars 16 to 40, VS 8 to 32, match text 36 to 52, names and life 44 to 68, pips 72 to 80, meta and banner 84 to 100. */
.versus-full .bar { top: 14px; width: 220px; height: 24px; }
.versus-full .bar.you { left: 24px; }
.versus-full .bar.opponent { right: 24px; }
.versus-full .vs { top: 6px; }
.versus-full .vs-mark { font-size: 24px; line-height: 24px; }
.versus-full .vs-match { font-size: 16px; line-height: 16px; margin-top: 4px; }
.versus-full .side { top: 42px; width: 220px; height: 24px; }
.versus-full .side.you { left: 24px; }
.versus-full .side.opponent { right: 24px; }
.versus-full .name { height: 24px; font-size: 24px; line-height: 24px; max-width: 180px; }
.versus-full .name.fit-long { font-size: 20px; }
.versus-full .name.fit-longer { font-size: 16px; letter-spacing: 0; }
.versus-full .life { font-size: 30px; line-height: 24px; }
.versus-full .pips { top: 70px; gap: 8px; }
.versus-full .pips.you { left: 24px; }
.versus-full .pips.opponent { right: 24px; }
.versus-full .pip { width: 22px; height: 8px; }
.versus-full .meta { top: 82px; height: 16px; font-size: 16px; line-height: 16px; }
.versus-full .banner { top: 82px; height: 16px; font-size: 14px; line-height: 16px; }
.versus-full .callout-title { font-size: 38px; line-height: 40px; }
.versus-full .callout-subtitle { font-size: 18px; line-height: 20px; margin-top: 4px; }
.versus-full .size-waiting .callout-title { font-size: 26px; line-height: 28px; }
.versus-full .size-trophied .callout-title { font-size: 44px; line-height: 44px; }
.versus-full .size-idle .callout-title { font-size: 32px; line-height: 32px; }
.versus-full .round-in { font-size: 24px; line-height: 28px; }
.versus-full .fight { font-size: 48px; line-height: 52px; }

/* Compact: 380 x 64. Bars, life and VS 8 to 24, pips 28 to 32, name row 40 to 56. */
.versus-compact .bar { top: 6px; width: 128px; height: 16px; }
.versus-compact .bar.you { left: 40px; }
.versus-compact .bar.opponent { right: 40px; }
.versus-compact .life { position: absolute; top: 6px; width: 24px; font-size: 22px; line-height: 16px; }
.versus-compact .life.you { left: 12px; }
.versus-compact .life.opponent { right: 12px; text-align: right; }
.versus-compact .vs { top: 6px; }
.versus-compact .vs-mark { font-size: 16px; line-height: 16px; }
.versus-compact .pips { top: 26px; gap: 4px; }
.versus-compact .pips.you { left: 40px; }
.versus-compact .pips.opponent { right: 40px; }
.versus-compact .pip { width: 12px; height: 4px; }
.row { position: absolute; top: 38px; left: 12px; right: 12px; height: 16px; display: grid; grid-template-columns: 1fr auto 1fr; align-items: baseline; gap: 8px; line-height: 16px; }
.rname { font-size: 16px; font-weight: 700; letter-spacing: 1px; text-transform: uppercase; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
.rname.opponent { color: #cfd6e4; text-align: right; }
.rname.fit-long { font-size: 14px; }
.rname.fit-longer { font-size: 13px; letter-spacing: 0; }
.mid { font-size: 16px; letter-spacing: 2px; color: #aab3c5; white-space: nowrap; }
.mid.gold { color: #ffd84a; }
.versus-compact .callout-title { font-size: 24px; line-height: 24px; }
.versus-compact .size-waiting .callout-title { font-size: 16px; line-height: 16px; }
.versus-compact .size-trophied .callout-title { font-size: 26px; line-height: 28px; }
.versus-compact .size-idle .callout-title { font-size: 19px; line-height: 20px; }
.versus-compact .ko { font-size: 30px; line-height: 32px; letter-spacing: 4px; }
.versus-compact .round-in { font-size: 16px; line-height: 16px; }
.versus-compact .round-big { font-size: 26px; line-height: 28px; }
.versus-compact .round-big small { font-size: 12px; line-height: 12px; margin-top: 2px; }
.versus-compact .fight { font-size: 28px; line-height: 32px; }

@keyframes low { 50% { filter: brightness(1.6); } }
@keyframes drain { 0%, 40% { opacity: 1; } 100% { opacity: 0; } }
@keyframes pulse { 50% { opacity: 0.45; } }
@keyframes banner { 50% { opacity: 0.55; } }
@keyframes round-in { 0% { opacity: 0; transform: translateX(-60px); } 12% { opacity: 1; transform: none; } 45% { opacity: 1; transform: none; } 55% { opacity: 0; transform: translateX(60px); } 100% { opacity: 0; } }
@keyframes fight { 0%, 55% { opacity: 0; transform: scale(2.6); } 63% { opacity: 1; transform: scale(0.9); } 68% { transform: scale(1.08); } 72% { transform: scale(1); } 90% { opacity: 1; transform: scale(1); } 100% { opacity: 0; transform: scale(1.4); } }
@keyframes ko { 0% { opacity: 0; transform: scale(3); } 12% { opacity: 1; transform: scale(0.92); } 18% { transform: scale(1.05); } 24% { transform: scale(1); } 85% { opacity: 1; } 100% { opacity: 0; } }
@keyframes ko-flash { 0% { opacity: 0.6; } 12% { opacity: 0; } 100% { opacity: 0; } }
@keyframes shake { 25% { transform: translate(-4px, 2px); } 50% { transform: translate(4px, -2px); } 75% { transform: translate(-3px, -1px); } 100% { transform: none; } }
@keyframes round-big { 0% { opacity: 0; transform: translateX(-80px); } 15% { opacity: 1; transform: none; } 80% { opacity: 1; transform: none; } 100% { opacity: 0; transform: scale(0.6); } }

@media (prefers-reduced-motion: reduce) {
  /* Callouts appear and disappear without movement: no shake, flash or fade (the card unmounts .play on its timer). */
  .versus .bar.low .hp, .versus .callout.pulse .callout-title, .versus .banner { animation: none; }
  .versus .hp { transition: none; }
  .round-in, .fight, .ko, .round-big, .play, .flash { animation: none; }
  .play[data-animation='ko'] { animation: none; }
  .ghost { display: none; }
  .fight { opacity: 1; }
  .round-in, .flash { display: none; }
}
</style>
