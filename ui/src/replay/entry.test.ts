import { describe, expect, it } from 'vitest'
import { previousBaseline, replayGameId, requestedGame, switchedGame } from './entry'

describe('replayGameId', () => {
  it('the entry reads ?game=', () => {
    expect(replayGameId('?game=12')).toBe(12)
    expect(replayGameId('?other=1&game=7')).toBe(7)
  })

  it('has no game for a missing or unusable one', () => {
    expect(replayGameId('')).toBeNull()
    expect(replayGameId('?game=')).toBeNull()
    expect(replayGameId('?game=abc')).toBeNull()
    expect(replayGameId('?game=0')).toBeNull()
    expect(replayGameId('?game=-3')).toBeNull()
    expect(replayGameId('?game=1.5')).toBeNull()
  })
})

describe('switchedGame', () => {
  it('switches to another game', () => {
    expect(switchedGame(1, 2)).toBe(2)
    expect(switchedGame(null, 4)).toBe(4)
  })

  it('does nothing for the game already shown', () => {
    expect(switchedGame(3, 3)).toBeNull()
  })

  it('opens the game again once the in-window picker moved off it', () => {
    expect(switchedGame(3, 3, true)).toBe(3)
    expect(switchedGame(3, 4, true)).toBe(4)
  })
})

describe('requestedGame', () => {
  it('shows the game the app says was asked for while the page loaded', () => {
    // A second Replay click landed before the page listened: the page shows game 1, the app has 2.
    expect(requestedGame(1, 2)).toBe(2)
  })

  it('does nothing when nothing was requested or the game is already shown', () => {
    expect(requestedGame(1, null)).toBeNull()
    expect(requestedGame(5, 5)).toBeNull()
  })
})

describe('previousBaseline', () => {
  it('turns the previous game sideboard into the viewer baseline, one card per copy', () => {
    const entry = { catalog_id: 5678, quantity: 2, name: 'Qcard', type: 'Instant', image: 'https://cards.example/q.jpg', local_image: 'q.jpg' }
    expect(previousBaseline({ game: 1, sideboard: [entry] })).toEqual({
      game: 1,
      cards: [
        { Id: -1, CatalogID: 5678, Zone: 'Sideboard', Owner: -1, name: 'Qcard', type: 'Instant', image: 'https://cards.example/q.jpg', local_image: 'q.jpg' },
        { Id: -2, CatalogID: 5678, Zone: 'Sideboard', Owner: -1, name: 'Qcard', type: 'Instant', image: 'https://cards.example/q.jpg', local_image: 'q.jpg' },
      ],
    })
  })

  it('has none for a first game', () => {
    expect(previousBaseline(null)).toBeNull()
  })
})
