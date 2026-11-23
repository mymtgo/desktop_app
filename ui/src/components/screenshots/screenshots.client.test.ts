import { describe, expect, it, vi } from 'vitest'
import type { CardStatView, DeckScreenshotView, RunCardView } from '@/bindings'
import { DEFAULT_VISIBILITY } from '@/lib/cardStats/columns'
import { htmlText, renderHtml } from '@/replay/replayTestHarness'

vi.mock('@/bindings', () => ({ commands: {} }))

const { default: DeckScreenshot } = await import('./DeckScreenshot.vue')
const { default: LeagueScreenshot } = await import('./LeagueScreenshot.vue')
const { default: CardStatScreenshot } = await import('./CardStatScreenshot.vue')
const { default: MatchupScreenshot } = await import('@/components/decks/MatchupScreenshot.vue')

/** 0.x's placeholder, written `-` in v1. */
const EM_DASH = String.fromCharCode(0x2014)

const BOLT = 'data:image/jpeg;base64,cWJvbHQ='

const deck: DeckScreenshotView = {
  name: 'Qburn',
  format: 'Modern',
  colors: ['R'],
  matchRecord: { wins: 3, losses: 1, draws: 0, total: 4, winRate: 75, label: '3 - 1' },
  coverArtBase64: 'data:image/jpeg;base64,Y292ZXI=',
  nonLandCards: [
    { name: 'Qgoblin', type: 'Creature', quantity: 2, imageBase64: null },
    { name: 'Qbolt', type: 'Instant', quantity: 4, imageBase64: BOLT },
  ],
  landCards: [{ name: 'Qmountain', type: 'Land', quantity: 18, imageBase64: null }],
  sideboardCards: [{ name: 'Qbolt', type: 'Instant', quantity: 2, imageBase64: BOLT }],
  cmcDistribution: [
    { cmc: '0', count: 0 },
    { cmc: '1', count: 4 },
    { cmc: '2', count: 2 },
    { cmc: '7+', count: 0 },
  ],
  typeDistribution: [
    { type: 'Creature', count: 2 },
    { type: 'Instant', count: 4 },
    { type: 'Land', count: 18 },
  ],
  commanderFormat: false,
}

function run(fields: Partial<RunCardView> = {}): RunCardView {
  return {
    key: 'log:1',
    source: 'log',
    name: 'Modern League',
    format: 'Modern',
    formatCode: 'CMODERN',
    state: 'complete',
    manual: false,
    startedAtMs: 0,
    droppedAtMs: null,
    deck: { id: 5, name: 'Qburn', colors: ['R'], coverArt: null, localCoverArt: null },
    versionLabel: null,
    results: ['W', 'L', 'W'],
    matches: [
      {
        token: 'q1',
        result: 'W',
        opponentName: 'Qfoe',
        opponentArchetype: 'Qcontrol',
        opponentArchetypeId: 1,
        games: [
          { result: 'W', onPlay: true },
          { result: 'W', onPlay: false },
        ],
        startedAtMs: 0,
        durationSeconds: 1500,
        notes: null,
        manual: false,
      },
      {
        token: 'q2',
        result: 'L',
        opponentName: null,
        opponentArchetype: null,
        opponentArchetypeId: null,
        games: [{ result: 'L', onPlay: null }],
        startedAtMs: 0,
        durationSeconds: null,
        notes: null,
        manual: false,
      },
    ],
    notes: null,
    classification: 'NONE',
    liveRound: null,
    avgMatchSeconds: null,
    timeOfDay: null,
    gameWins: 0,
    gameLosses: 0,
    onPlayRecord: { wins: 0, losses: 0 },
    onDrawRecord: { wins: 0, losses: 0 },
    tixDelta: null,
    topOpponentArchetype: null,
    topMatchups: [],
    limited: false,
    colors: null,
    ...fields,
  } as RunCardView
}

function stat(fields: Partial<CardStatView>): CardStatView {
  return new Proxy(
    {
      key: 'q-oracle',
      oracleId: 'q-oracle',
      name: 'Qbolt',
      colorIdentity: 'R',
      type: 'Instant',
      image: null,
      localImage: null,
      isSideboard: true,
      ...fields,
    } as Record<string, unknown>,
    { get: (target, key) => (key in target ? target[key as string] : 0) },
  ) as unknown as CardStatView
}

async function render(component: Parameters<typeof renderHtml>[0], props: Record<string, unknown>) {
  const { html, warnings } = await renderHtml(component, props)
  expect(warnings).toEqual([])
  return { html, text: htmlText(html) }
}

describe('the screenshots (0.x DeckScreenshot, LeagueScreenshot, CardStatScreenshot, MatchupScreenshot)', () => {
  it('the deck screenshot shows its fields and inlined images', async () => {
    const { html, text } = await render(DeckScreenshot, { data: deck })
    for (const part of ['Qburn', 'Modern', 'Creature 2', 'Instant 4', 'Land 18', '75%', 'Win Rate', '3 - 1', 'Record']) {
      expect(text).toContain(part)
    }
    expect(text).toContain('Sideboard')
    expect(text).toContain('mymtgo.com')
    expect(text).toContain('24 cards main · 2 sideboard')
    expect(html).toContain('src="data:image/jpeg;base64,Y292ZXI="')
    expect(html.split(`src="${BOLT}"`).length - 1).toBe(6)
    expect(html).not.toContain(EM_DASH)
  })

  it('the deck screenshot of a Brawl deck puts the commander block before the main deck', async () => {
    const { html, text } = await render(DeckScreenshot, { data: { ...deck, format: 'Historic Brawl', commanderFormat: true } })
    expect(text).not.toContain('Sideboard')
    expect(text).toContain('Commander')
    expect(html).toContain('data-section="commander"')
    // Laid out first (flex order) with its divider on its right.
    expect(html).toContain('order:-1')
  })

  it('the deck screenshot of an MTGO Brawl deck keeps its sideboard', async () => {
    const { text } = await render(DeckScreenshot, { data: { ...deck, format: 'Brawl', commanderFormat: false } })
    expect(text).toContain('Sideboard')
    expect(text).not.toContain('Commander')
  })

  it('the deck screenshot leaves the sideboard and cover out when there are none', async () => {
    const { text, html } = await render(DeckScreenshot, {
      data: { ...deck, coverArtBase64: null, sideboardCards: [] },
    })
    expect(text).not.toContain('Sideboard')
    expect(html).not.toContain('Y292ZXI=')
  })

  it('the league screenshot shows the score and each match', async () => {
    const { html, text } = await render(LeagueScreenshot, {
      league: run(),
      coverArtBase64: 'data:image/jpeg;base64,Y292ZXI=',
    })
    for (const part of ['Qburn', 'Modern', '2-1', 'Result', 'Opponent', 'Vs', 'Game 1', 'Game 3', 'Time']) {
      expect(text).toContain(part)
    }
    expect(text).toContain('1 Win Qfoe Qcontrol Win (OTP) Win (OTD) - 25m')
    expect(text).toContain('2 Loss - Unknown Loss - - -')
    expect(text).toContain('mymtgo.com')
    expect(html).toContain('src="data:image/jpeg;base64,Y292ZXI="')
    expect(html).not.toContain(EM_DASH)
  })

  it('the league screenshot names an unknown deck', async () => {
    const { text, html } = await render(LeagueScreenshot, { league: run({ deck: null }) })
    expect(text).toContain('Unknown Deck')
    expect(html).not.toContain('<img')
  })

  it('the league screenshot names a limited run by its set and kind', async () => {
    const { text } = await render(LeagueScreenshot, {
      league: run({ deck: null, limited: true, name: 'QNW Draft', format: 'Draft', colors: ['U', 'R'] }),
    })
    expect(text).toContain('QNW Draft')
    expect(text).not.toContain('Unknown Deck')
  })

  it('the card stat screenshot shows the card, chips, versus and blocks', async () => {
    const { html, text } = await render(CardStatScreenshot, {
      stat: stat({ totalGames: 10, keptGames: 6, keptWon: 4, keptLost: 2 }),
      visibleColumns: DEFAULT_VISIBILITY,
      timeframeLabel: 'Last 30 days',
      archetypeName: 'Qcontrol',
      archetypeColorIdentity: 'U,W',
      boardLabel: 'Postboard',
      playDrawLabel: 'On the play',
      imageDataUrl: BOLT,
    })
    for (const part of ['Qbolt', 'Sideboard', 'Instant', 'Versus', 'Qcontrol', 'Postboard', 'On the play', 'Last 30 days']) {
      expect(text).toContain(part)
    }
    expect(text).toContain('Kept 60% (6)')
    expect(text).toContain('Kept Win 67% (6)')
    expect(text).toContain('Games 10')
    expect(text).toContain('mymtgo.com')
    expect(html).toContain(`src="${BOLT}"`)
    expect(html).toContain('color:#22c55e')
    expect(html).not.toContain(EM_DASH)
  })

  it('the card stat screenshot without an image or archetype', async () => {
    const { html, text } = await render(CardStatScreenshot, {
      stat: stat({ isSideboard: false }),
      visibleColumns: DEFAULT_VISIBILITY,
      timeframeLabel: 'All time',
    })
    expect(text).not.toContain('Versus')
    expect(text).not.toContain('Sideboard')
    expect(html).not.toContain('<img')
  })

  it('the matchup screenshot (Plan T) still renders its fields', async () => {
    const { text } = await render(MatchupScreenshot, {
      matchup: { name: 'Qcontrol', colors: ['U'], matches: 4, matchWinRate: 75, matchRecord: '3 - 1' },
      detail: { perGame: [{ gameNumber: 1, winRate: 50, record: '2 - 2' }], otpWinRate: 60, avgTurns: 7.25, avgMulligans: null },
      timeframeLabel: 'All time',
    })
    expect(text).toContain('Qcontrol')
    expect(text).toContain('4 matches · All time')
    expect(text).toContain('Game 1 50% 2 - 2')
    expect(text).toContain('7.3')
  })
})
