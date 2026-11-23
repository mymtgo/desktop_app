import { describe, expect, it } from 'vitest'
import { pageToShow, showingText } from './paging'

describe('pageToShow', () => {
  it('keeps a page that has rows', () => {
    expect(pageToShow({ page: 2, rows: 3, total: 53, perPage: 50 })).toBeNull()
  })

  it('keeps an empty first page', () => {
    expect(pageToShow({ page: 1, rows: 0, total: 0, perPage: 50 })).toBeNull()
  })

  it('moves an empty page beyond the last to the last page', () => {
    expect(pageToShow({ page: 3, rows: 0, total: 100, perPage: 50 })).toBe(2)
    expect(pageToShow({ page: 2, rows: 0, total: 50, perPage: 50 })).toBe(1)
  })

  it('moves to the first page when no matches remain', () => {
    expect(pageToShow({ page: 4, rows: 0, total: 0, perPage: 50 })).toBe(1)
  })

  it('always moves backwards, even when the total is stale', () => {
    expect(pageToShow({ page: 2, rows: 0, total: 500, perPage: 50 })).toBe(1)
  })
})

describe('showingText', () => {
  it('names the rows on the page and the total, as 0.x', () => {
    expect(showingText({ page: 1, perPage: 50, total: 120 })).toBe('Showing 1–50 of 120 matches')
    expect(showingText({ page: 3, perPage: 50, total: 120 })).toBe('Showing 101–120 of 120 matches')
  })

  it('is empty without rows', () => {
    expect(showingText({ page: 1, perPage: 50, total: 0 })).toBe('')
  })
})
