import { describe, expect, it, vi } from 'vitest'
import { findAll, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

vi.mock('@/bindings', () => ({ commands: {} }))

const { default: HelpPopoverBody } = await import('./HelpPopoverBody.vue')

function body(extra: Record<string, unknown> = {}) {
  const mounted = mountFake(HelpPopoverBody, { busy: false, error: null, ...extra })
  const buttons = findAll(mounted.root, (node) => node.tag === 'button')
  const button = (label: string): FakeNode | undefined => buttons.find((node) => textOf(node).trim() === label)
  return { text: textOf(mounted.root).replace(/\s+/g, ' '), button }
}

describe('the Get help popover (0.x HelpPopover)', () => {
  it("shows 0.x's strings and v1's privacy sentence", () => {
    const { text, button } = body()
    expect(text).toContain('Need help?')
    expect(text).toContain(
      'Hop into our Discord server to report bugs or ask questions. Including your report data helps a lot when reporting an issue.',
    )
    expect(button('Open Discord')).toBeDefined()
    expect(button('Download report data')).toBeDefined()
    expect(text).toContain(
      "This zips your MTGO and Arena logs and MyMTGO's recent logs to help with debugging. You can optionally upload it in Discord. It includes your MTGO and Arena usernames, the names of players you met, and file paths that may show your computer's account name.",
    )
  })

  it('says Preparing… and disables the download while busy', () => {
    const { button } = body({ busy: true })
    const download = button('Preparing…')
    expect(download).toBeDefined()
    expect(download!.props.disabled).toBeTruthy()
    expect(button('Download report data')).toBeUndefined()
  })

  it('shows the no log files error with Open settings, in place of the helper text', () => {
    const { text, button } = body({
      error: { message: "Couldn't find log files: check your log path in settings.", showSettingsLink: true },
    })
    expect(text).toContain("Couldn't find log files: check your log path in settings.")
    expect(button('Open settings')).toBeDefined()
    expect(text).not.toContain('This zips your MTGO')
  })

  it('shows the generic error without the settings link', () => {
    const { text, button } = body({
      error: { message: 'Something went wrong building the report bundle.', showSettingsLink: false },
    })
    expect(text).toContain('Something went wrong building the report bundle.')
    expect(button('Open settings')).toBeUndefined()
  })

  it('emits from its buttons', () => {
    const onOpenDiscord = vi.fn()
    const onDownload = vi.fn()
    const onOpenSettings = vi.fn()
    const { button } = body({
      onOpenDiscord,
      onDownload,
      onOpenSettings,
      error: { message: 'x', showSettingsLink: true },
    })
    ;(button('Open Discord')!.props.onClick as () => void)()
    ;(button('Download report data')!.props.onClick as () => void)()
    ;(button('Open settings')!.props.onClick as () => void)()
    expect(onOpenDiscord).toHaveBeenCalledTimes(1)
    expect(onDownload).toHaveBeenCalledTimes(1)
    expect(onOpenSettings).toHaveBeenCalledTimes(1)
  })
})
