import { describe, expect, it, vi } from 'vitest'
import type { OverlayArtwork, OverlaySettingsView, Settings } from '@/bindings'
import type { PreviewSample } from '@/components/overlay/overlaySamples'
import { DEFAULT_DISPLAY } from '@/lib/leagueOverlayCard'
import { previewState } from '@/lib/overlaySettings'
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

// The real sheet teleports into the document; plain elements that render their slots stand in,
// keeping the attributes the slide-out passes (modal, overlay, outside-click handling).
vi.mock('@/components/ui/sheet', async () => {
  const { defineComponent, h } = await import('vue')
  const plain = (tag: string) =>
    defineComponent({ inheritAttrs: false, setup: (_props, { slots }) => () => h(tag, slots.default?.()) })
  const keeping = (tag: string) =>
    defineComponent({ inheritAttrs: false, setup: (_props, { slots, attrs }) => () => h(tag, { ...attrs }, slots.default?.()) })
  return {
    Sheet: keeping('div'),
    SheetContent: keeping('aside'),
    SheetHeader: plain('div'),
    SheetTitle: plain('h2'),
    SheetDescription: plain('p'),
  }
})

const { default: EventCardSheet } = await import('./EventCardSheet.vue')

const WITH_STRIP = { full: { width: 332, height: 128 }, compact: { width: 272, height: 106 } }
const WITHOUT_STRIP = { full: { width: 332, height: 112 }, compact: { width: 272, height: 90 } }

function view(obsSizes = WITH_STRIP): OverlaySettingsView {
  return {
    deck: { id: 7, name: 'Qgreen Tron', archetype: null, colorIdentity: ['G'], label: null },
    deckArtUrl: null,
    backgroundUrl: null,
    artwork: 'deck',
    publicUrl: 'https://mymtgo.com/overlays/Qstream',
    obsSizes,
    publish: { lastPublishedAtMs: null, error: null },
    signedIn: true,
    overlayWindowsAvailable: true,
  }
}

function mount(options: { artwork?: OverlayArtwork; eventStrip?: boolean; sizesUpdating?: boolean } = {}) {
  const changes: Array<Partial<Settings>> = []
  const previews: PreviewSample[] = []
  const eventStrip = options.eventStrip ?? true
  const pageView = view(eventStrip ? WITH_STRIP : WITHOUT_STRIP)
  const kinds: string[] = []
  const samples: PreviewSample[] = []
  const mounted = mountFake(EventCardSheet, {
    open: true,
    previewKind: 'league',
    previewSample: 'in_game',
    'onUpdate:previewKind': (kind: string) => kinds.push(kind),
    'onUpdate:previewSample': (sample: PreviewSample) => samples.push(sample),
    preview: previewState('in_game', pageView, 'full', DEFAULT_DISPLAY, 'classic', 1_800_000_000_000),
    now: 1_800_000_000_000,
    artwork: options.artwork ?? 'deck',
    size: 'full',
    theme: 'classic',
    display: DEFAULT_DISPLAY,
    finishedMinutes: 5,
    eventStrip,
    view: pageView,
    sizesUpdating: options.sizesUpdating ?? false,
    onChange: (change: Partial<Settings>) => changes.push(change),
    onPreview: (sample: PreviewSample) => previews.push(sample),
  })
  expect(mounted.warnings).toEqual([])
  return { root: mounted.root, changes, previews, kinds, samples }
}

/** The element children of a node, text and comments left out. */
function elements(node: FakeNode): FakeNode[] {
  return node.children.filter((child) => child.tag !== '#text' && child.tag !== '#comment')
}

function byProp(root: FakeNode, key: string, value: string): FakeNode {
  const found = findAll(root, (node) => node.props[key] === value)
  expect(found, `${key}=${value}`).toHaveLength(1)
  return found[0]
}

describe('the event card slide-out', () => {
  it('shows the custom background directly under Artwork, only for Custom artwork', () => {
    expect(findAll(mount().root, (node) => node.props['data-reveal'] === 'artwork')).toEqual([])

    const { root } = mount({ artwork: 'custom' })
    const artwork = byProp(root, 'aria-labelledby', 'event-card-artwork')
    const block = byProp(root, 'data-reveal', 'artwork')
    const siblings = elements(artwork.parent!)
    expect(siblings[siblings.indexOf(artwork) + 1]).toBe(block)
    expect(textOf(block)).toContain('Custom background')
    expect(textOf(block)).toContain('Upload')
  })

  it('shows Artwork and Games directly under Theme for Classic', () => {
    const { root } = mount()
    const theme = byProp(root, 'aria-labelledby', 'event-card-theme')
    const siblings = elements(theme.parent!)
    const revealed = siblings[siblings.indexOf(theme) + 1]
    expect(revealed.props['data-reveal']).toBe('theme')
    expect(textOf(revealed)).toContain('Artwork')
    expect(textOf(revealed)).toContain('Games')
  })

  it('tags Standing, and only Standing, as Challenges', () => {
    const { root } = mount()
    const labels = findAll(root, (node) => typeof node.props.for === 'string' && String(node.props.for).startsWith('event-card-show-'))
    const tagged = labels.filter((label) => textOf(label).includes('Challenges'))
    expect(tagged.map((label) => label.props.for)).toEqual(['event-card-show-standing'])
    expect(textOf(tagged[0])).toContain('Standing')
  })

  it('previews a Challenge state that shows the Standing it switches', () => {
    const { root, changes, previews } = mount()
    const standing = byProp(root, 'id', 'event-card-show-standing')
    ;(standing.props.onToggle as (on: boolean) => void)(true)
    expect(changes).toEqual([{ overlayLeagueDisplay: { ...DEFAULT_DISPLAY, standing: true } }])
    expect(previews).toEqual(['challenge_round'])

    // A part of both kinds leaves the preview alone.
    const record = byProp(root, 'id', 'event-card-show-record')
    ;(record.props.onToggle as (on: boolean) => void)(false)
    expect(previews).toEqual(['challenge_round'])
  })

  it('gives the card and OBS sizes under Size, with and without the strip', () => {
    expect(textOf(mount().root)).toContain('Card 300 × 80. OBS Browser Source 332 × 128. Changing this changes the OBS size.')
    expect(textOf(mount({ eventStrip: false }).root)).toContain('Card 300 × 80. OBS Browser Source 332 × 112.')
    const updating = textOf(mount({ sizesUpdating: true }).root)
    expect(updating).toContain('Card 300 × 80. OBS Browser Source size updating.')
    expect(updating).not.toContain('332 ×')
  })

  it('saves the other event strip switch', () => {
    const { root, changes } = mount()
    const strip = byProp(root, 'id', 'event-card-strip')
    expect(strip.props.checked).toBe(true)
    ;(strip.props.onToggle as (on: boolean) => void)(false)
    expect(changes).toEqual([{ overlayEventStrip: false }])
    expect(textOf(root)).toContain('Adds 16 px to the OBS Browser Source (20 px on Versus Full).')
  })

  it('keeps every control usable while a change saves', () => {
    const { root } = mount()
    expect(findAll(root, (node) => node.props.disabled === true)).toEqual([])
  })

  it('shows the preview title as plain text, not a second label box', () => {
    const inputs = findAll(mount().root, (node) => node.tag === 'input')
    expect(inputs.map((input) => input.props.id)).toEqual(['event-card-finished-minutes'])
  })

  it('is a modal sheet over the dimmed app that a click outside closes', () => {
    const { root } = mount()
    const sheet = findAll(root, (node) => node.tag === 'div' && 'modal' in node.props)
    expect(sheet, 'no non-modal root').toEqual([])
    const content = findAll(root, (node) => node.tag === 'aside')[0]
    expect(content.props.overlay).toBeUndefined()
    expect(content.props.onInteractOutside, 'outside clicks are not held back').toBeUndefined()
    expect(content.props.onEscapeKeyDown).toBeUndefined()
  })

  it('has its own preview pickers, bound to the shared preview state', () => {
    const { root, kinds, samples } = mount()
    const text = textOf(root)
    expect(text).toContain('Sample data')
    expect(text).not.toContain('follows the preview on the page')
    const challenge = findAll(root, (node) => node.tag === 'button' && textOf(node).trim() === 'Challenge')
    expect(challenge).toHaveLength(1)
    ;(challenge[0].props.onClick as () => void)()
    expect(kinds).toEqual(['tournament'])
    expect(samples).toEqual(['challenge_round'])
  })

  it('opens with focus on the chosen kind', () => {
    const { root } = mount()
    const content = findAll(root, (node) => node.tag === 'aside')[0]
    const focused: string[] = []
    let prevented = false
    const chosen = { focus: () => focused.push('chosen') }
    const event = {
      target: { querySelector: (selector: string) => (selector === '[aria-pressed="true"]' ? chosen : null) },
      preventDefault: () => (prevented = true),
    }
    ;(content.props.onOpenAutoFocus as (event: unknown) => void)(event)
    expect(focused).toEqual(['chosen'])
    expect(prevented).toBe(true)
  })
})
