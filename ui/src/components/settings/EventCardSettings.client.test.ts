import { describe, expect, it, vi } from 'vitest'
import type { LeagueDisplay, LeagueTheme, OverlaySettingsView } from '@/bindings'
import { DEFAULT_DISPLAY } from '@/lib/leagueOverlayCard'
import { findAll, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

// The real switch needs a DOM; a plain element stands in (as BackgroundCard's test does).
vi.mock('@/components/ui/switch', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Switch: defineComponent({
      props: { modelValue: Boolean, disabled: Boolean, id: String },
      emits: ['update:modelValue'],
      setup: (props, { emit }) => () =>
        h('switch', {
          checked: props.modelValue,
          disabled: props.disabled,
          id: props.id,
          onToggle: (on: boolean) => emit('update:modelValue', on),
        }),
    }),
  }
})

// The real input binds DOM listeners; a plain element stands in.
vi.mock('@/components/ui/input', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Input: defineComponent({
      props: { modelValue: [String, Number], id: String },
      setup: (props) => () => h('input', { value: props.modelValue, id: props.id }),
    }),
  }
})

// The real select needs a DOM; plain elements that render their slots stand in.
vi.mock('@/components/ui/select', async () => {
  const { defineComponent, h } = await import('vue')
  const plain = (tag: string) =>
    defineComponent({ inheritAttrs: false, setup: (_props, { slots }) => () => h(tag, slots.default?.()) })
  return {
    Select: plain('select'),
    SelectContent: plain('div'),
    SelectGroup: plain('optgroup'),
    SelectItem: plain('option'),
    SelectLabel: plain('label'),
    SelectSeparator: plain('hr'),
    SelectTrigger: plain('div'),
    SelectValue: plain('span'),
  }
})

// The real sheet teleports into the document; plain elements that render their slots stand in.
vi.mock('@/components/ui/sheet', async () => {
  const { defineComponent, h } = await import('vue')
  const plain = (tag: string) =>
    defineComponent({ inheritAttrs: false, setup: (_props, { slots }) => () => h(tag, slots.default?.()) })
  return {
    Sheet: plain('div'),
    SheetContent: plain('aside'),
    SheetHeader: plain('div'),
    SheetTitle: plain('h2'),
    SheetDescription: plain('p'),
  }
})

const { default: EventCardSettings } = await import('./EventCardSettings.vue')

const KARN = 'https://cards.scryfall.io/art_crop/karn.jpg'

function view(): OverlaySettingsView {
  return {
    deck: { id: 7, name: 'Qgreen Tron', archetype: null, colorIdentity: ['G'], label: null },
    deckArtUrl: KARN,
    backgroundUrl: null,
    artwork: 'deck',
    publicUrl: 'https://mymtgo.com/overlays/Qstream',
    obsSizes: { full: { width: 332, height: 128 }, compact: { width: 272, height: 106 } },
    publish: { lastPublishedAtMs: null, error: null },
    signedIn: true,
    overlayWindowsAvailable: true,
  }
}

function mount(display: LeagueDisplay, theme: LeagueTheme = 'classic') {
  const mounted = mountFake(EventCardSettings, {
    artwork: 'deck',
    size: 'full',
    theme,
    display,
    finishedMinutes: 5,
    eventStrip: true,
    view: view(),
    sizesUpdating: false,
  })
  expect(mounted.warnings).toEqual([])
  return mounted
}

function render(display: LeagueDisplay, theme: LeagueTheme = 'classic'): string {
  return textOf(mount(display, theme).root)
}

/** The page's preview well: the controls, the card and the caption (the slide-out left out). */
function pageText(root: FakeNode): string {
  const sheet = findAll(root, (node) => node.tag === 'aside')
  return textOf(root).replace(textOf(sheet[0]), '')
}

/** The one button with this label on the page, or in the slide-out with `inSheet`. */
function button(root: FakeNode, label: string, inSheet = false): FakeNode {
  const sheet = findAll(root, (node) => node.tag === 'aside')[0]
  const scope = inSheet ? sheet : root
  const found = findAll(scope, (node) => node.tag === 'button' && textOf(node).trim() === label).filter(
    (node) => inSheet || !findAll(sheet, (inner) => inner === node).length,
  )
  expect(found, label).toHaveLength(1)
  return found[0]
}

describe('the Event card settings', () => {
  it('opens the slide-out from Customise and the caption', () => {
    const { root } = mount(DEFAULT_DISPLAY)
    const text = pageText(root)
    expect(text).toContain('Customise')
    expect(text).toContain('Classic · Full · Deck cover · 300 × 80 (OBS 332 × 128)')
    expect(button(root, 'Classic · Full · Deck cover · 300 × 80 (OBS 332 × 128)').props.onClick).toBeTypeOf('function')
  })

  it('groups the league states and switches to the Challenge states', async () => {
    const mounted = mount(DEFAULT_DISPLAY)
    const league = pageText(mounted.root)
    for (const label of ['Playing', 'Between matches', 'Finished', 'In game', 'Trophied']) expect(league).toContain(label)
    expect(league).not.toContain('Swiss round')

    ;(button(mounted.root, 'Challenge').props.onClick as () => void)()
    await mounted.settle()
    const challenge = pageText(mounted.root)
    expect(challenge).toContain('Swiss round')
    expect(challenge).toContain('Missed the cut')
    expect(challenge).not.toContain('Trophied')
  })

  it('switches the preview to a Challenge when Standing is switched', async () => {
    const mounted = mount(DEFAULT_DISPLAY)
    const standing = findAll(mounted.root, (node) => node.props.id === 'event-card-show-standing')[0]
    ;(standing.props.onToggle as (on: boolean) => void)(true)
    await mounted.settle()
    expect(pageText(mounted.root)).toContain('Swiss round')
  })

  it('lists the games style and the parts in the slide-out', () => {
    const text = render(DEFAULT_DISPLAY)
    for (const label of ['Games', 'Dots', 'Letters', 'Event name', 'Format', 'Archetype', 'Progress', 'Status', 'Record', 'Standing']) {
      expect(text).toContain(label)
    }
    expect(text).toContain('Finished event shown for')
    expect(text).toContain('Other event strip')
  })

  it('previews the chosen parts', () => {
    expect(pageText(mount(DEFAULT_DISPLAY).root).toLowerCase()).toContain('modern')
    const bare = pageText(mount({ ...DEFAULT_DISPLAY, event: false, format: false, progress: false }).root)
    expect(bare.toLowerCase()).not.toContain('modern')
    expect(bare).not.toContain('Match 5/5')
  })

  it('groups the look and the parts, without the window switch', () => {
    const text = render(DEFAULT_DISPLAY)
    expect(text).toContain('Event card')
    expect(text).toContain('Look')
    expect(text).toContain('Show on the card')
    expect(text).not.toContain('Desktop window')
  })

  it('offers the themes and hides artwork and games for versus', () => {
    const classic = render(DEFAULT_DISPLAY)
    expect(classic).toContain('Theme')
    expect(classic).toContain('Versus')
    expect(classic).toContain('Artwork')
    expect(classic).toContain('Dots')
    const versus = render(DEFAULT_DISPLAY, 'versus')
    expect(versus).not.toContain('Artwork')
    expect(versus).not.toContain('Dots')
  })

  it('changes the page preview from the pickers in the slide-out', async () => {
    const mounted = mount(DEFAULT_DISPLAY)
    ;(button(mounted.root, 'Challenge', true).props.onClick as () => void)()
    await mounted.settle()
    expect(pageText(mounted.root)).toContain('Swiss round')
  })
})
