import { describe, expect, it, vi } from 'vitest'
import type { LicenceList } from '@/lib/licences'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})
vi.mock('@/content/licenceList', () => ({
  licenceList: {
    packages: [
      { name: 'serde', version: '1.0.200', licence: 'MIT OR Apache-2.0', ecosystem: 'rust', texts: [0, 1] },
      { name: 'quiet-crate', version: '0.1.0', licence: 'MIT', ecosystem: 'rust', texts: [] },
      { name: 'vue', version: '3.5.43', licence: 'MIT', ecosystem: 'ui', texts: [0] },
    ],
    texts: ['The MIT text', 'The Apache text'],
  } satisfies LicenceList,
}))

const { default: LicencesPage } = await import('./LicencesPage.vue')

function rowButton(root: ReturnType<typeof mountFake>['root'], name: string) {
  return findAll(root, (node) => node.tag === 'button').find((node) => textOf(node).includes(name))
}

describe('the third-party licences page', () => {
  it('lists every package with its version and licence, Rust crates and UI packages apart', () => {
    const mounted = mountFake(LicencesPage)
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('Third-party licences')
    expect(text).toContain('Rust crates (2)')
    expect(text).toContain('Interface packages (1)')
    expect(text).toContain('serde')
    expect(text).toContain('1.0.200')
    expect(text).toContain('MIT OR Apache-2.0')
    expect(text).toContain('vue')
    expect(text).not.toContain('The MIT text')
    const back = findAll(mounted.root, (node) => node.tag === 'a')[0]
    expect(back.props.href).toBe('/settings/advanced')
  })

  it('expands an entry to show its licence texts and collapses it again', async () => {
    const mounted = mountFake(LicencesPage)
    ;(rowButton(mounted.root, 'serde')!.props.onClick as () => void)()
    await mounted.settle()
    let text = textOf(mounted.root)
    expect(text).toContain('The MIT text')
    expect(text).toContain('The Apache text')
    expect(rowButton(mounted.root, 'serde')!.props['aria-expanded']).toBe(true)
    ;(rowButton(mounted.root, 'serde')!.props.onClick as () => void)()
    await mounted.settle()
    text = textOf(mounted.root)
    expect(text).not.toContain('The MIT text')
  })

  it('says when a package ships no licence text', async () => {
    const mounted = mountFake(LicencesPage)
    ;(rowButton(mounted.root, 'quiet-crate')!.props.onClick as () => void)()
    await mounted.settle()
    expect(textOf(mounted.root)).toContain('This package includes no licence file. Its licence is MIT.')
  })
})
