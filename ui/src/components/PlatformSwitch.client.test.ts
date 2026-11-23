import { describe, expect, it } from 'vitest'
import type { PlatformContextView, PlatformView } from '@/bindings'
import { findAll, hasClass, mountFake } from '@/replay/replayTestHarness'
import PlatformSwitch from './PlatformSwitch.vue'

function context(fields: Partial<PlatformContextView> = {}): PlatformContextView {
  return { current: 'mtgo', mtgo: true, arena: true, endstep: false, switchable: true, ...fields }
}

function mount(props: { context: PlatformContextView; live?: PlatformView[] }) {
  const mounted = mountFake(PlatformSwitch, { live: [], ...props })
  expect(mounted.warnings).toEqual([])
  return mounted
}

const buttons = (mounted: ReturnType<typeof mount>) => findAll(mounted.root, (node) => node.tag === 'button')

describe('PlatformSwitch', () => {
  it('renders nothing when not switchable', () => {
    const mounted = mount({ context: context({ arena: false, switchable: false }) })
    expect(buttons(mounted)).toHaveLength(0)
  })

  it('has both logos with their alt text', () => {
    const mounted = mount({ context: context() })
    const alts = findAll(mounted.root, (node) => node.tag === 'img').map((node) => node.props.alt)
    expect(alts).toEqual(['MTGO', 'MTG Arena'])
  })

  it('adds the Endstep option, with its own logo, when Endstep is on', () => {
    const mounted = mount({ context: context({ endstep: true }) })
    const images = findAll(mounted.root, (node) => node.tag === 'img')
    expect(images.map((node) => node.props.alt)).toEqual(['MTGO', 'MTG Arena', 'Endstep'])
    // Small SVGs are inlined, so the logo is recognised by its diamond path.
    expect(decodeURIComponent(String(images[2]!.props.src))).toContain('M32 11 L49 28 L32 45 L15 28 Z')
    expect(buttons(mounted)).toHaveLength(3)
  })

  it('has no Endstep option while Endstep is off', () => {
    const mounted = mount({ context: context({ endstep: false }) })
    expect(findAll(mounted.root, (node) => node.tag === 'img').map((node) => node.props.alt)).not.toContain('Endstep')
  })

  it('leaves out Arena while Arena is off', () => {
    const mounted = mount({ context: context({ arena: false, endstep: true }) })
    expect(findAll(mounted.root, (node) => node.tag === 'img').map((node) => node.props.alt)).toEqual(['MTGO', 'Endstep'])
  })

  it('marks the current platform', () => {
    const mounted = mount({ context: context({ current: 'arena' }) })
    expect(buttons(mounted).map((node) => node.props['aria-pressed'])).toEqual([false, true])
  })

  it('emits switch with the other platform on click', () => {
    const switched: string[] = []
    const mounted = mountFake(PlatformSwitch, {
      context: context(),
      live: [],
      onSwitch: (platform: string) => switched.push(platform),
    })
    const [mtgo, arena] = buttons(mounted)
    ;(mtgo!.props.onClick as () => void)()
    expect(switched).toEqual([])
    ;(arena!.props.onClick as () => void)()
    expect(switched).toEqual(['arena'])
  })

  it('shows the dot only on the other logos with a live match', () => {
    const dots = (mounted: ReturnType<typeof mount>) =>
      buttons(mounted).map((button) => findAll(button, (node) => node.props['data-live-dot'] !== undefined).length)
    const all = context({ endstep: true })
    expect(dots(mount({ context: all, live: ['arena'] }))).toEqual([0, 1, 0])
    expect(dots(mount({ context: { ...all, current: 'arena' }, live: ['endstep'] }))).toEqual([0, 0, 1])
    expect(dots(mount({ context: { ...all, current: 'arena' }, live: ['arena'] }))).toEqual([0, 0, 0])
    expect(dots(mount({ context: all, live: ['arena', 'endstep'] }))).toEqual([0, 1, 1])
    expect(dots(mount({ context: all, live: [] }))).toEqual([0, 0, 0])
    const mounted = mount({ context: all, live: ['arena'] })
    expect(hasClass(findAll(mounted.root, (node) => node.props['data-live-dot'] !== undefined)[0]!, 'bg-warning')).toBe(true)
  })
})
