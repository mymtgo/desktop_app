import { describe, expect, it } from 'vitest'
import type { DrawOdds, GameOverlayView, OverlayArchetypeOption, SideboardGuide, SidedInCard, SidedOutCard } from '@/bindings'
import {
  EMPTY_CELL,
  FLASH_MS,
  changedRows,
  clampSample,
  cutCell,
  drawChance,
  drawOddsEmpty,
  expandsOnTabPick,
  hasTabSections,
  isCurrent,
  isNewer,
  maxSample,
  moveHighlight,
  nextDrawsLabel,
  otherNoteCaption,
  overlayTabs,
  pct,
  pickerList,
  revealsEmpty,
  sideboardEmpty,
  sideboardHeader,
  sidedInFaded,
  sidedInName,
  sidedOutHeading,
  sidedOutName,
  tabAfter,
  toggleLabel,
  winLossCell,
} from './overlay'

const sections = { opponent: true, drawOdds: true, reveals: true, sideboard: true }

function view(overrides: Partial<GameOverlayView> = {}): GameOverlayView {
  return {
    sections,
    hasMatch: true,
    hasDeck: true,
    hasArchetype: true,
    format: 'Modern',
    collapsed: false,
    opponent: null,
    game: null,
    drawOdds: null,
    reveals: null,
    potential: null,
    sideboard: null,
    notes: { current: [], other: [] },
    isSideboarding: false,
    ...overrides,
  }
}

function odds(librarySize: number, cards: DrawOdds['cards'] = []): DrawOdds {
  return { cards, librarySize, liveLibraryCount: librarySize }
}

function oddsCard(catalogId: number, name: string, remaining: number): DrawOdds['cards'][number] {
  return {
    catalogId,
    name,
    cardType: 'Instant',
    colorIdentity: null,
    image: null,
    artCrop: null,
    localImage: null,
    localArtCrop: null,
    remaining,
    total: 4,
  }
}

function sidedIn(overrides: Partial<SidedInCard> = {}): SidedInCard {
  return {
    oracleId: 'o-rip',
    name: 'Rest in Peace',
    cardType: 'Enchantment',
    colorIdentity: null,
    image: null,
    artCrop: null,
    localImage: null,
    localArtCrop: null,
    quantity: 2,
    sidedInGames: 0,
    wins: 0,
    losses: 0,
    winrate: null,
    recommended: false,
    plannedQuantity: null,
    stale: false,
    ...overrides,
  }
}

function sidedOut(overrides: Partial<SidedOutCard> = {}): SidedOutCard {
  return {
    oracleId: 'o-bolt',
    name: 'Lightning Bolt',
    cardType: 'Instant',
    image: null,
    artCrop: null,
    localImage: null,
    localArtCrop: null,
    quantity: 4,
    sidedOutGames: 0,
    recommended: false,
    plannedQuantity: null,
    stale: false,
    ...overrides,
  }
}

function guide(overrides: Partial<SideboardGuide> = {}): SideboardGuide {
  return { sidedIn: [], sidedOut: [], postboardGames: 0, postboardRecord: '0 - 0', hasPlan: false, ...overrides }
}

describe('pct', () => {
  it('never rounds an uncertain chance to 0% or 100%', () => {
    expect(pct(0.9999)).toBe('99.9%')
    expect(pct(0.00001)).toBe('0.1%')
    expect(pct(0.9982, 0)).toBe('99%')
  })

  it('shows the exact bounds as they are', () => {
    expect(pct(0)).toBe('0.0%')
    expect(pct(1)).toBe('100.0%')
    expect(pct(0.5)).toBe('50.0%')
  })
})

describe('the draw stepper', () => {
  it('stops at the library left, at least one', () => {
    expect(maxSample(odds(24))).toBe(24)
    expect(maxSample(odds(0))).toBe(1)
    expect(maxSample(null)).toBe(1)
  })

  it('is pulled back when the library shrinks below it', () => {
    expect(clampSample(30, 22)).toBe(22)
    expect(clampSample(5, 22)).toBe(5)
    expect(clampSample(0, 22)).toBe(1)
  })

  it('says draw or draws', () => {
    expect(nextDrawsLabel(1)).toBe('Next draw')
    expect(nextDrawsLabel(3)).toBe('Next draws')
  })

  it('gives a card with no copies left no chance', () => {
    expect(drawChance(24, 0, 3)).toBe(0)
    expect(drawChance(24, 4, 1)).toBeCloseTo(4 / 24)
  })
})

describe('the flash', () => {
  it('marks a row whose copies changed, never a new one', () => {
    const previous = new Map<string, number>()
    expect(changedRows(previous, [oddsCard(102, 'Lightning Bolt', 4)])).toEqual([])
    expect(changedRows(previous, [oddsCard(102, 'Lightning Bolt', 3), oddsCard(101, 'Mountain', 20)])).toEqual(['102'])
    expect(changedRows(previous, [oddsCard(102, 'Lightning Bolt', 3), oddsCard(101, 'Mountain', 20)])).toEqual([])
    // A card shuffled back flashes too.
    expect(changedRows(previous, [oddsCard(102, 'Lightning Bolt', 4)])).toEqual(['102'])
    expect(FLASH_MS).toBe(1200)
  })
})

describe('the empty messages', () => {
  it('says why the draw odds have nothing to list', () => {
    expect(drawOddsEmpty(null)).toBe('Waiting for match…')
    expect(drawOddsEmpty(odds(0))).toBe('No deck data for this match')
    expect(drawOddsEmpty(odds(1, [oddsCard(1, 'Q', 1)]))).toBeNull()
  })

  it('says why the reveals have nothing to list', () => {
    expect(revealsEmpty(view({ hasMatch: false }))).toBe('Waiting for match…')
    expect(revealsEmpty(view({ reveals: [] }))).toBe('Nothing revealed yet')
    expect(
      revealsEmpty(
        view({
          reveals: [],
          potential: {
            maindeck: [
              {
                catalogId: 1,
                name: 'Q',
                cardType: 'Instant',
                image: null,
                artCrop: null,
                localImage: null,
                localArtCrop: null,
                quantity: 1,
              },
            ],
            sideboard: [],
          },
        }),
      ),
    ).toBeNull()
  })

  it('says why there is no sideboard guide, in 0.x order', () => {
    expect(sideboardEmpty(view({ hasMatch: false, hasDeck: false, hasArchetype: false }))).toBe('Waiting for match…')
    expect(sideboardEmpty(view({ hasDeck: false, hasArchetype: false }))).toBe('No deck linked to this match')
    expect(sideboardEmpty(view({ hasArchetype: false }))).toBe('Pick an archetype to see your sideboard guide')
    expect(sideboardEmpty(view())).toBeNull()
  })
})

describe('the sideboard guide', () => {
  it('heads the guide with the postboard games and record', () => {
    expect(sideboardHeader(guide())).toBe('No games vs this archetype yet')
    expect(sideboardHeader(guide({ hasPlan: true }))).toBe('Your guide · No games vs this archetype yet')
    expect(sideboardHeader(guide({ postboardGames: 1, postboardRecord: '1 - 0' }))).toBe('1 post-board game · 1 - 0 overall')
    expect(sideboardHeader(guide({ postboardGames: 5, postboardRecord: '3 - 2', hasPlan: true }))).toBe(
      'Your guide · 5 post-board games · 3 - 2 overall',
    )
  })

  it('writes the empty cells as a hyphen, never an em dash', () => {
    expect(EMPTY_CELL).toBe('-')
    expect(winLossCell(sidedIn())).toBe('-')
    expect(cutCell(sidedOut())).toBe('-')
    expect(winLossCell(sidedIn({ sidedInGames: 3, wins: 2, losses: 1, winrate: 67 }))).toBe('2–1 (67%)')
    expect(cutCell(sidedOut({ sidedOutGames: 4 }))).toBe('4×')
  })

  it('marks stale plan rows and fades them', () => {
    expect(sidedInName(sidedIn({ stale: true }))).toBe('Rest in Peace (not in sideboard)')
    expect(sidedOutName(sidedOut({ stale: true }))).toBe('Lightning Bolt (not in maindeck)')
    expect(sidedInFaded(sidedIn({ stale: true }), true)).toBe(true)
    expect(sidedInFaded(sidedIn(), false)).toBe(true)
    expect(sidedInFaded(sidedIn(), true)).toBe(false)
    expect(sidedInFaded(sidedIn({ sidedInGames: 1 }), false)).toBe(false)
  })

  it('heads the cut list by scope', () => {
    expect(sidedOutHeading(guide({ sidedOut: [sidedOut()] }))).toBe('Usually cut (1)')
    expect(sidedOutHeading(guide({ hasPlan: true, sidedOut: [sidedOut()] }))).toBe('Take out (1)')
  })
})

describe('the tabs', () => {
  it('lists the sections on, in 0.x order', () => {
    expect(overlayTabs(sections).map((tab) => tab.label)).toEqual(['Draw odds', 'Revealed', 'Sideboarding'])
    expect(overlayTabs({ ...sections, drawOdds: false }).map((tab) => tab.value)).toEqual(['reveals', 'sideboard'])
    expect(hasTabSections({ ...sections, drawOdds: false, reveals: false, sideboard: false })).toBe(false)
  })

  it('expands a collapsed overlay when a tab is picked', () => {
    expect(expandsOnTabPick(true)).toBe(true)
    expect(expandsOnTabPick(false)).toBe(false)
    expect(toggleLabel(true)).toBe('Expand overlay')
    expect(toggleLabel(false)).toBe('Collapse overlay')
  })

  it('the_sideboard_tab_opens_once_when_sideboarding_starts', () => {
    let tab = 'draw-odds'
    let sideboarding = false
    const push = (next: GameOverlayView) => {
      tab = tabAfter(tab, sideboarding, next)
      sideboarding = next.isSideboarding
    }
    push(view())
    expect(tab).toBe('draw-odds')
    push(view({ isSideboarding: true }))
    expect(tab).toBe('sideboard')
    // The player goes back to the draw odds while still sideboarding.
    tab = 'draw-odds'
    push(view({ isSideboarding: true }))
    expect(tab).toBe('draw-odds')
    // Leaving sideboarding never moves the tab.
    tab = 'sideboard'
    push(view())
    expect(tab).toBe('sideboard')
    // With the section off, nothing opens.
    tab = 'draw-odds'
    sideboarding = false
    push(view({ isSideboarding: true, sections: { ...sections, sideboard: false } }))
    expect(tab).toBe('draw-odds')
  })
})

describe('pushes', () => {
  it('applies only a newer revision', () => {
    expect(isNewer(1, null)).toBe(true)
    expect(isNewer(3, 2)).toBe(true)
    expect(isNewer(2, 2)).toBe(false)
    expect(isNewer(1, 2)).toBe(false)
  })
})

describe('the archetype picker', () => {
  const option = (id: number | null, uuid: string, name: string, isFallback = false): OverlayArchetypeOption => ({
    id,
    uuid,
    name,
    format: isFallback ? null : 'modern',
    colors: [],
    isFallback,
  })
  const options = [
    option(1, 'q-blink', 'Esper Blink'),
    option(null, 'q-unmet', 'Amulet Titan'),
    option(9, 'q-homebrew', 'Homebrew', true),
  ]

  it('searches the archetypes, never the fallbacks', () => {
    expect(pickerList(options, '').map((o) => o.name)).toEqual(['Esper Blink', 'Amulet Titan', 'Homebrew'])
    expect(pickerList(options, ' ESPER ').map((o) => o.name)).toEqual(['Esper Blink', 'Homebrew'])
    expect(pickerList(options, 'zzz').map((o) => o.name)).toEqual(['Homebrew'])
  })

  it('wraps the highlight', () => {
    expect(moveHighlight(2, 1, 3)).toBe(0)
    expect(moveHighlight(0, -1, 3)).toBe(2)
    expect(moveHighlight(0, 1, 0)).toBe(0)
  })

  it('ticks the archetype shown, held by id, unmet by uuid', () => {
    expect(isCurrent(options[0], 1, 'q-blink')).toBe(true)
    expect(isCurrent(options[0], 2, 'q-blink')).toBe(false)
    expect(isCurrent(options[1], null, 'q-unmet')).toBe(true)
    expect(isCurrent(options[1], null, null)).toBe(false)
  })
})

describe('notes', () => {
  it('captions another deck note with its deck and date', () => {
    const ms = Date.UTC(2026, 6, 1, 12)
    expect(otherNoteCaption('Qburn', ms, 'en-GB')).toBe('Qburn · 1 Jul 2026')
  })
})
