import { describe, expect, it } from 'vitest'
import type { WrappedBundle, WrappedCard as Card } from '@/bindings'
import { TEST_BUNDLES } from '@/lib/wrapped/testBundles'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

const { default: WrappedCard } = await import('./WrappedCard.vue')

function cardOf(bundle: WrappedBundle, kind: Card['kind']): Card {
  const found = bundle.cards.find((card) => card.kind === kind)
  if (!found) throw new Error(`no ${kind} card`)
  return found
}

function mount(bundle: WrappedBundle, kind: Card['kind']) {
  return mountFake(WrappedCard, { card: cardOf(bundle, kind), bundle })
}

describe('a Wrapped card', () => {
  it('every card in the grinder profile renders without warnings', () => {
    const bundle = TEST_BUNDLES.grinder
    for (const card of bundle.cards) {
      const mounted = mountFake(WrappedCard, { card, bundle })
      expect(mounted.warnings, card.kind).toEqual([])
      expect(textOf(mounted.root).trim().length, card.kind).toBeGreaterThan(0)
      mounted.unmount()
    }
  })

  it('a card with no art renders no img', () => {
    const noArt = { remote: null, local: null, custom: null }
    const card = { ...cardOf(TEST_BUNDLES.grinder, 'sideboardHero'), art: noArt, image: noArt }
    const mounted = mountFake(WrappedCard, { card, bundle: TEST_BUNDLES.grinder })
    expect(findAll(mounted.root, (node) => node.tag === 'img')).toEqual([])
    expect(textOf(mounted.root)).toContain('Wear // Tear')
  })

  it('art that fails to load collapses to the no-art layout', async () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'formats')
    const imgs = findAll(mounted.root, (node) => node.tag === 'img')
    expect(imgs).toHaveLength(1)
    const handlers = imgs[0].props.onError
    for (const handler of Array.isArray(handlers) ? handlers : [handlers]) (handler as () => void)()
    await mounted.settle()
    expect(findAll(mounted.root, (node) => node.tag === 'img')).toEqual([])
  })

  it('a long name is clamped to two lines', () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'deckOfYear')
    const titles = findAll(mounted.root, (node) => hasClass(node, 'wrapped-title'))
    expect(titles).toHaveLength(1)
    expect(textOf(titles[0])).toContain('Boros Energy (the one with too many Phlage)')
    expect(hasClass(titles[0], 'line-clamp-2')).toBe(true)
  })

  it('the opening card draws one panel per set, then the winners sentence', () => {
    const mounted = mount(TEST_BUNDLES.medium, 'magicYear')
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-set'))).toHaveLength(7)
    expect(findAll(mounted.root, (node) => node.tag === 'img' && hasClass(node, 'wrapped-set-icon'))).toHaveLength(7)
    expect(findAll(mounted.root, (node) => node.tag === 'strong')).toHaveLength(4)
    expect(textOf(mounted.root)).toContain('Simic Neoform')
  })

  it('the banned card shows each banned card with its formats', () => {
    const mounted = mount(TEST_BUNDLES.medium, 'banned')
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-banned-card'))).toHaveLength(10)
    expect(textOf(mounted.root)).toContain('Vintage (restricted)')
  })

  it('the numbers card draws a square per day under its figures', () => {
    const mounted = mount(TEST_BUNDLES.medium, 'yearInNumbers')
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-day'))).toHaveLength(334)
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-figure'))).toHaveLength(4)
    expect(textOf(mounted.root)).toContain('the top 34% of MyMTGO players')
  })

  it('the MVP lists its runners up under a hairline, with their impact', () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'mvpCard')
    const rows = findAll(mounted.root, (node) => hasClass(node, 'wrapped-runner-up'))
    expect(rows).toHaveLength(3)
    expect(textOf(rows[0])).toContain('+7')
    expect(textOf(mounted.root)).toContain('Also pulling their weight')
  })

  it('a card with no runners up has no honourable mentions', () => {
    const card = { ...cardOf(TEST_BUNDLES.grinder, 'mvpCard'), runnersUp: [] }
    const mounted = mountFake(WrappedCard, { card, bundle: TEST_BUNDLES.grinder })
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-runners-up'))).toEqual([])
  })

  it('the trophy cabinet shows a trophy per league won and a medal per Top 8', () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'trophies')
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-trophy'))).toHaveLength(2)
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-medal'))).toHaveLength(1)
  })

  it('the Limited card shows the colour pair and the best run', () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'limited')
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-limited-colours'))).toHaveLength(1)
    expect(textOf(mounted.root)).toContain('Golgari')
    expect(textOf(mounted.root)).toContain('3-0')
  })

  it('the player type card lists what earned it at the foot', () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'playerType')
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-persona-line'))).toHaveLength(3)
    expect(textOf(mounted.root)).toContain('The Grinder')
  })

  it('the closing card recaps the year and can start again', async () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'wrap')
    expect(textOf(mounted.root)).toContain('1,284')
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-wrap-sets'))).toHaveLength(1)
  })

  it('a deck card without winRate shows no win rate', () => {
    const mounted = mount(TEST_BUNDLES.light, 'deckOfYear')
    expect(textOf(mounted.root)).not.toContain('%')
    expect(textOf(mounted.root)).toContain('21')
  })

  it('the toughest rival never shows its win rate', () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'toughestRival')
    const text = textOf(mounted.root)
    expect(text).not.toContain('41%')
    expect(text).toContain('you still beat them 17 times')
  })

  it('the opening card has the deck of the year art blurred behind it, other cards do not', () => {
    const opening = mount(TEST_BUNDLES.grinder, 'yearInNumbers')
    const backdrops = findAll(opening.root, (node) => hasClass(node, 'wrapped-backdrop'))
    expect(backdrops).toHaveLength(1)
    expect(findAll(backdrops[0], (node) => node.tag === 'img')).toHaveLength(1)
    const formats = mount(TEST_BUNDLES.grinder, 'formats')
    expect(findAll(formats.root, (node) => hasClass(node, 'wrapped-backdrop'))).toEqual([])
  })

  it('no deck art means no backdrop', () => {
    const bundle = { ...TEST_BUNDLES.grinder, cards: TEST_BUNDLES.grinder.cards.filter((card) => card.kind !== 'deckOfYear') }
    const mounted = mountFake(WrappedCard, { card: cardOf(bundle, 'yearInNumbers'), bundle })
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-backdrop'))).toEqual([])
  })

  it('the formats card lists the other formats as honourable mentions', () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'formats')
    const text = textOf(mounted.root)
    expect(text).toContain('Honourable mentions')
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-mention'))).toHaveLength(3)
    expect(text).toContain('231 matches')
    expect(findAll(mounted.root, (node) => node.tag === 'img')).toHaveLength(1)
  })

  it('when you play draws its scene behind the words', () => {
    for (const [profile, period] of [
      ['weekender', 'morning'],
      ['limited', 'afternoon'],
      ['medium', 'evening'],
      ['grinder', 'night'],
    ] as const) {
      const mounted = mount(TEST_BUNDLES[profile], 'whenYouPlay')
      const scenes = findAll(mounted.root, (node) => hasClass(node, 'wrapped-scene'))
      expect(scenes, profile).toHaveLength(1)
      expect(scenes[0].props['data-period'], profile).toBe(period)
      expect(findAll(scenes[0], (node) => node.tag === 'img'), profile).toHaveLength(1)
      expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-credit')), profile).toHaveLength(1)
      expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-fact')), profile).toHaveLength(2)
      mounted.unmount()
    }
  })

  it('other cards have no scene', () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'comebacks')
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-scene'))).toEqual([])
  })

  it('a deck card shows its colours, its key cards fanned and its win rate', () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'deckOfYear')
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-colors'))).toHaveLength(1)
    expect(findAll(mounted.root, (node) => node.tag === 'img' && hasClass(node, 'wrapped-key-card'))).toHaveLength(5)
    const figures = findAll(mounted.root, (node) => hasClass(node, 'wrapped-figure')).map((node) => textOf(node))
    expect(figures).toEqual(['412 matches', '239 wins', '58% win rate', '1 trophy'])
    expect(textOf(mounted.root)).toContain('Your deck of the year')
  })

  it('a deck card under 50% shows only its matches', () => {
    const mounted = mount(TEST_BUNDLES.light, 'deckOfYear')
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-figure'))).toHaveLength(1)
    expect(textOf(mounted.root)).not.toContain('win')
  })

  it('the MVP card features the card itself and its impact', () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'mvpCard')
    const images = findAll(mounted.root, (node) => node.tag === 'img' && hasClass(node, 'wrapped-spotlight-image'))
    expect(images).toHaveLength(1)
    expect(images[0].props.alt).toBe('Ragavan, Nimble Pilferer')
    const text = textOf(mounted.root)
    expect(text).toContain('+9')
    expect(text).toContain('points of impact')
    expect(text).toContain('When you cast it you won 64% of games, 9 points above your usual.')
  })

  it('the opponent archetype cards are drawn like the deck cards', () => {
    for (const kind of ['mostFaced', 'bestMatchup', 'toughestRival'] as const) {
      const mounted = mount(TEST_BUNDLES.grinder, kind)
      expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-colors')), kind).toHaveLength(1)
      expect(findAll(mounted.root, (node) => node.tag === 'img' && hasClass(node, 'wrapped-key-card')), kind).toHaveLength(5)
      expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-figure')).length, kind).toBeGreaterThan(1)
      mounted.unmount()
    }
  })

  it('the opening hands and sideboard cards feature the card itself', () => {
    const opener = mount(TEST_BUNDLES.grinder, 'openingHand')
    expect(findAll(opener.root, (node) => node.tag === 'img' && hasClass(node, 'wrapped-spotlight-image'))).toHaveLength(1)
    expect(textOf(opener.root)).toContain('41%')
    expect(textOf(opener.root)).toContain('of your opening hands')
    const hero = mount(TEST_BUNDLES.grinder, 'sideboardHero')
    expect(findAll(hero.root, (node) => node.tag === 'img' && hasClass(node, 'wrapped-spotlight-image'))).toHaveLength(1)
    expect(textOf(hero.root)).toContain('15% of your games after sideboarding')
  })

  it('the comebacks card shows its art with the artist credited', () => {
    const mounted = mount(TEST_BUNDLES.grinder, 'comebacks')
    expect(textOf(mounted.root)).toContain('Never give up, never surrender')
    // Only the faint art behind: no card, it is not the player's card.
    expect(findAll(mounted.root, (node) => node.tag === 'img' && hasClass(node, 'wrapped-spotlight-image'))).toEqual([])
    expect(findAll(mounted.root, (node) => node.tag === 'img' && hasClass(node, 'wrapped-spotlight-art'))).toHaveLength(1)
    expect(textOf(mounted.root)).toContain('games won after a mulligan')
    const credits = findAll(mounted.root, (node) => hasClass(node, 'wrapped-credit'))
    expect(credits).toHaveLength(1)
    expect(textOf(credits[0])).toContain('Milivoj Ćeran')
  })
})
