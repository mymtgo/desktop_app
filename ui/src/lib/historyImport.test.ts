import { describe, expect, it } from 'vitest'
import type { HistoryDeckVersionView, HistoryScanStatus } from '@/bindings'
import {
  DEFAULT_MIN_CONFIDENCE,
  SORT_OPTIONS,
  confidenceColor,
  confidenceText,
  deckVersionGroups,
  deckVersionLabel,
  setDeckDescription,
  settableDeckVersions,
  emptyLine,
  formatScanDate,
  importAllFilters,
  importResultLine,
  isStaleStatus,
  minConfidenceLabel,
  outcomeVariant,
  pageStateFor,
  progressLine,
  progressPercent,
  scanQuery,
  stageLabel,
} from './historyImport'

function status(overrides: Partial<HistoryScanStatus> = {}): HistoryScanStatus {
  return {
    scanId: 3,
    deckVersionId: 7,
    deckName: 'Qdeck',
    status: 'processing',
    stage: 'decoding',
    progress: 10,
    total: 40,
    error: null,
    matchCount: 0,
    formats: [],
    confidenceStats: null,
    ...overrides,
  }
}

describe('confidence', () => {
  it('is green at or above 0.8, yellow at or above 0.6, red below and muted when unscored', () => {
    expect(confidenceColor(1)).toBe('text-green-500')
    expect(confidenceColor(0.8)).toBe('text-green-500')
    expect(confidenceColor(0.79)).toBe('text-yellow-500')
    expect(confidenceColor(0.6)).toBe('text-yellow-500')
    expect(confidenceColor(0.59)).toBe('text-red-500')
    expect(confidenceColor(0)).toBe('text-red-500')
    expect(confidenceColor(null)).toBe('text-muted-foreground')
  })

  it('reads as a whole percentage, or a dash when unscored', () => {
    expect(confidenceText(0.67)).toBe('67%')
    expect(confidenceText(1)).toBe('100%')
    expect(confidenceText(0)).toBe('0%')
    expect(confidenceText(0.29)).toBe('29%')
    expect(confidenceText(null)).toBe('-')
  })
})

describe('min confidence slider', () => {
  it('starts at 0.1, which hides matches without a game log', () => {
    expect(DEFAULT_MIN_CONFIDENCE).toBe(0.1)
  })

  it('says Off at 0, Any up to 0.1, and a percentage above', () => {
    expect(minConfidenceLabel(0)).toBe('Off')
    expect(minConfidenceLabel(0.1)).toBe('Any')
    expect(minConfidenceLabel(0.2)).toBe('20%+')
    expect(minConfidenceLabel(0.30000000000000004)).toBe('30%+')
    expect(minConfidenceLabel(0.7)).toBe('70%+')
    expect(minConfidenceLabel(1)).toBe('100%+')
  })
})

describe('results query', () => {
  it('sorts by confidence, date or opponent as 0.x offered', () => {
    expect(SORT_OPTIONS).toEqual([
      { value: 'confidence', label: 'Confidence (high first)' },
      { value: 'started_at', label: 'Date (newest first)' },
      { value: 'opponent', label: 'Opponent (A-Z)' },
    ])
  })

  it('sends a blank format and an Off slider as absent, descending by default', () => {
    expect(scanQuery({ format: '', minConfidence: 0, sort: 'confidence' }, 1)).toEqual({
      format: null,
      minConfidence: null,
      sort: 'confidence',
      dir: 'desc',
      page: 1,
      perPage: null,
    })
  })

  it('keeps a chosen format and minimum, and sorts opponents A to Z', () => {
    expect(scanQuery({ format: 'Modern', minConfidence: 0.6, sort: 'opponent' }, 4)).toEqual({
      format: 'Modern',
      minConfidence: 0.6,
      sort: 'opponent',
      dir: 'asc',
      page: 4,
      perPage: null,
    })
    expect(scanQuery({ format: '', minConfidence: 0.1, sort: 'started_at' }, 2).dir).toBe('desc')
  })

  it('imports all through the same filters', () => {
    expect(importAllFilters({ format: '', minConfidence: 0, sort: 'confidence' })).toEqual({
      format: null,
      minConfidence: null,
    })
    expect(importAllFilters({ format: 'Pauper', minConfidence: 0.1, sort: 'opponent' })).toEqual({
      format: 'Pauper',
      minConfidence: 0.1,
    })
  })

  it('says which filters left the table empty', () => {
    expect(emptyLine('', 0)).toBe('No matches found.')
    expect(emptyLine('Modern', 0)).toBe('No matches found for this format.')
    expect(emptyLine('', 0.1)).toBe('No matches found at this confidence level.')
    expect(emptyLine('Modern', 0.5)).toBe('No matches found for this format at this confidence level.')
  })
})

describe('scan progress', () => {
  it('labels each stage as 0.x did', () => {
    expect(stageLabel('discovering')).toBe('Discovering game logs...')
    expect(stageLabel('decoding')).toBe('Decoding game logs...')
    expect(stageLabel('parsing')).toBe('Parsing match history...')
    expect(stageLabel('scoring')).toBe('Scoring matches...')
    expect(stageLabel('something new')).toBe('Processing...')
  })

  it('shows progress as done of total with a rounded percentage', () => {
    expect(progressPercent(0, 0)).toBe(0)
    expect(progressPercent(1, 3)).toBe(33)
    expect(progressPercent(2, 3)).toBe(67)
    expect(progressPercent(40, 40)).toBe(100)
    expect(progressLine(2, 3)).toBe('2 / 3 (67%)')
  })

  it('starts the page from the scan the app holds', () => {
    expect(pageStateFor(null)).toBe('setup')
    expect(pageStateFor(status({ status: 'processing' }))).toBe('processing')
    expect(pageStateFor(status({ status: 'complete' }))).toBe('results')
    expect(pageStateFor(status({ status: 'cancelled' }))).toBe('setup')
    expect(pageStateFor(status({ status: 'failed' }))).toBe('setup')
  })

  it('ignores a status from an older scan, or one taking an ended scan back to processing', () => {
    const shown = status({ scanId: 3, status: 'complete' })
    expect(isStaleStatus(null, status({ scanId: 1 }))).toBe(false)
    expect(isStaleStatus(shown, status({ scanId: 2, status: 'cancelled' }))).toBe(true)
    // A poll sent before the scan completed can answer after its complete push.
    expect(isStaleStatus(shown, status({ scanId: 3, status: 'processing' }))).toBe(true)
    expect(isStaleStatus(shown, status({ scanId: 3, status: 'complete' }))).toBe(false)
    expect(isStaleStatus(status({ scanId: 3, status: 'processing' }), status({ scanId: 3 }))).toBe(false)
    expect(isStaleStatus(shown, status({ scanId: 4, status: 'processing' }))).toBe(false)
    expect(isStaleStatus(shown, null)).toBe(false)
  })
})

describe('results table', () => {
  it('shows a win as the default badge and anything else as destructive', () => {
    expect(outcomeVariant('win')).toBe('default')
    expect(outcomeVariant('loss')).toBe('destructive')
    expect(outcomeVariant('draw')).toBe('destructive')
  })

  it('formats the day with the month by name in the viewer timezone', () => {
    // 2026-03-04 23:30 UTC is already 5 March in Sydney.
    const ms = Date.UTC(2026, 2, 4, 23, 30)
    expect(formatScanDate(ms, 'UTC')).toBe('4 Mar 2026')
    expect(formatScanDate(ms, 'Australia/Sydney')).toBe('5 Mar 2026')
    expect(formatScanDate(Date.UTC(2026, 8, 15, 12), 'UTC')).toBe('15 Sep 2026')
    expect(formatScanDate(null, 'UTC')).toBe('-')
  })

  it('reports what an import stored and skipped', () => {
    expect(importResultLine({ imported: 3, skipped: 0 })).toBe('3 match(es) imported.')
    expect(importResultLine({ imported: 1, skipped: 2 })).toBe(
      '1 match(es) imported. 2 skipped: already stored, or no start time or account.',
    )
  })
})

describe('deck versions', () => {
  const version: HistoryDeckVersionView = {
    id: 1,
    deckName: 'Qburn',
    archived: false,
    modifiedAt: '04/03/2026',
    format: 'Modern',
    currentAccount: true,
    limited: false,
  }

  it('names a deck with its format and date, and an archived one as deleted', () => {
    expect(deckVersionLabel(version)).toBe('Qburn (Modern, 04/03/2026)')
    expect(deckVersionLabel({ ...version, archived: true })).toBe('Qburn (deleted) (04/03/2026)')
  })
})

describe('the deck version picker groups (issue #332)', () => {
  const at = (id: number, archived: boolean): HistoryDeckVersionView => ({
    id,
    deckName: `Q${id}`,
    archived,
    modifiedAt: '04/03/2026',
    format: 'Modern',
    currentAccount: true,
    limited: false,
  })

  it('lists active decks before deleted ones, keeping the order, and leaves out an empty group', () => {
    expect(deckVersionGroups([at(1, false), at(2, true), at(3, false)])).toEqual([
      { label: 'Active Decks', versions: [at(1, false), at(3, false)] },
      { label: 'Deleted Decks', versions: [at(2, true)] },
    ])
    expect(deckVersionGroups([at(1, false)])).toEqual([{ label: 'Active Decks', versions: [at(1, false)] }])
    expect(deckVersionGroups([])).toEqual([])
  })

  it('offers only the current account\'s constructed decks', () => {
    const theirs = { ...at(2, false), currentAccount: false }
    const pool = { ...at(3, false), limited: true }
    expect(settableDeckVersions([at(1, false), theirs, pool, at(4, true)])).toEqual([at(1, false), at(4, true)])
  })

  it('says what "Set deck" changes for one match or several', () => {
    expect(setDeckDescription(1)).toBe('Choose the deck version this match was played with.')
    expect(setDeckDescription(3)).toBe('Choose the deck version the 3 selected matches were played with.')
  })
})
