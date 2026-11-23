import { describe, expect, it, vi } from 'vitest'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

// The real switch needs a DOM; a plain element stands in (as BackgroundCard's test does).
vi.mock('@/components/ui/switch', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Switch: defineComponent({
      props: { modelValue: Boolean, disabled: Boolean, id: String },
      setup: (props) => () => h('switch', { checked: props.modelValue, disabled: props.disabled, id: props.id }),
    }),
  }
})

const { default: EventOutputsCard } = await import('./EventOutputsCard.vue')

const WITH_STRIP = { full: { width: 332, height: 128 }, compact: { width: 272, height: 106 } }
const WITHOUT_STRIP = { full: { width: 332, height: 112 }, compact: { width: 272, height: 90 } }

function render(note: string | null = null, strip = true): string {
  const mounted = mountFake(EventOutputsCard, {
    windowEnabled: true,
    windowDisabled: note !== null,
    note,
    enabled: true,
    url: 'https://mymtgo.com/overlays/Qstream',
    size: 'full',
    theme: 'classic',
    obsSizes: strip ? WITH_STRIP : WITHOUT_STRIP,
    strip,
    publish: { lastPublishedAtMs: null, error: null },
    signedIn: true,
    disabled: false,
  })
  expect(mounted.warnings).toEqual([])
  return textOf(mounted.root)
}

describe('where the event card shows', () => {
  it('lists the desktop window and the stream overlay', () => {
    const text = render()
    expect(text).toContain('Where it shows')
    expect(text).toContain('Desktop window')
    expect(text).toContain('Stream overlay (OBS)')
    expect(text).toContain('https://mymtgo.com/overlays/Qstream')
  })

  it('says why the window is unavailable', () => {
    expect(render('Overlay windows are not available on Wayland.')).toContain('not available on Wayland')
  })

  it('gives the size from Rust and the resize note while the strip switch is on', () => {
    const on = render()
    expect(on).toContain('332 × 128')
    expect(on).toContain('Copy size (332×128)')
    expect(on).toContain('New in 1.4: the source is 16 px taller to fit the other event strip.')
    const off = render(null, false)
    expect(off).toContain('332 × 112')
    expect(off).not.toContain('New in 1.4')
  })

  it('never shows an old size while a size change is saving', () => {
    const mounted = mountFake(EventOutputsCard, {
      windowEnabled: true,
      windowDisabled: false,
      note: null,
      enabled: true,
      url: 'https://mymtgo.com/overlays/Qstream',
      size: 'full',
      theme: 'classic',
      obsSizes: null,
      strip: true,
      publish: { lastPublishedAtMs: null, error: null },
      signedIn: true,
      disabled: false,
    })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('(updating)')
    expect(text).not.toMatch(/\d+ × \d+/)
  })

  it('names each switch by its label', () => {
    const mounted = mountFake(EventOutputsCard, {
      windowEnabled: true,
      windowDisabled: false,
      note: null,
      enabled: true,
      url: null,
      size: 'full',
      theme: 'classic',
      obsSizes: WITH_STRIP,
      strip: true,
      publish: { lastPublishedAtMs: null, error: null },
      signedIn: true,
      disabled: false,
    })
    const labelled = findAll(mounted.root, (node) => typeof node.props.for === 'string').map((node) => node.props.for)
    const switches = findAll(mounted.root, (node) => node.tag === 'switch').map((node) => node.props.id)
    expect(labelled).toEqual(['event-output-window', 'event-output-obs'])
    expect(switches).toEqual(labelled)
  })
})
