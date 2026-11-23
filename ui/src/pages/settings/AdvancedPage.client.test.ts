import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { AppInfo } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const opened = vi.hoisted(() => ({ count: 0, console: 0, folder: 0 }))

vi.mock('@/bindings', () => ({
  commands: {
    openLogFolder: async () => {
      opened.count += 1
      return { status: 'ok', data: null }
    },
    openConsoleWindow: async () => {
      opened.console += 1
      return { status: 'ok', data: null }
    },
    openDatabaseFolder: async () => {
      opened.folder += 1
      return { status: 'ok', data: null }
    },
  },
}))
vi.mock('@/lib/queries', () => ({
  useAppInfo: () => ({
    data: ref<AppInfo>({
      version: '1.0.0-beta.2',
      dataDir: '/home/q/.local/share/mymtgo',
      fatal: null,
      trayAvailable: false,
      productName: 'MyMTGO',
      prerelease: true,
      os: 'windows',
    }),
  }),
}))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add: () => {} }) }))
vi.mock('@/components/settings/AboutCard.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({ props: { version: String }, setup: (props) => () => h('about-card', { version: props.version }) }),
  }
})

const { default: AdvancedPage } = await import('./AdvancedPage.vue')

describe('the Advanced settings page', () => {
  // 0.x Pages/AdvancedControllerTest "renders the advanced settings page with debug mode and version"
  // (debug mode deferred by the owner, #286).
  it('shows the Logs card with the log folder and the version footer', () => {
    const mounted = mountFake(AdvancedPage)
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('Logs')
    expect(text).toContain('/home/q/.local/share/mymtgo/logs')
    expect(text).toContain('MyMTGO v1.0.0-beta.2')
    expect(text).not.toContain('Debug Mode')
  })

  // Release spec section 9: About sits on the Advanced page (spec ruling 4).
  it('shows the About card with the version', () => {
    const mounted = mountFake(AdvancedPage)
    const about = findAll(mounted.root, (node) => node.tag === 'about-card')
    expect(about).toHaveLength(1)
    expect(about[0].props.version).toBe('1.0.0-beta.2')
  })

  it('opens the log folder from its button', async () => {
    const mounted = mountFake(AdvancedPage)
    const button = findAll(mounted.root, (node) => node.tag === 'button').find(
      (node) => textOf(node).trim() === 'Open folder',
    )
    expect(button).toBeDefined()
    ;(button!.props.onClick as () => void)()
    await mounted.settle()
    expect(opened.count).toBe(1)
  })

  it('opens the data console from its button', async () => {
    const mounted = mountFake(AdvancedPage)
    expect(textOf(mounted.root)).toContain('Data console')
    const button = findAll(mounted.root, (node) => node.tag === 'button').find(
      (node) => textOf(node).trim() === 'Open data console',
    )
    expect(button).toBeDefined()
    ;(button!.props.onClick as () => void)()
    await mounted.settle()
    expect(opened.console).toBe(1)
  })

  it('opens the database folder from its button', async () => {
    const mounted = mountFake(AdvancedPage)
    const button = findAll(mounted.root, (node) => node.tag === 'button').find(
      (node) => textOf(node).trim() === 'Open database folder',
    )
    expect(button).toBeDefined()
    ;(button!.props.onClick as () => void)()
    await mounted.settle()
    expect(opened.folder).toBe(1)
  })
})
