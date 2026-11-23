import { beforeEach, describe, expect, it, vi } from 'vitest'

const commands = { saveReportBundle: vi.fn() }
vi.mock('@/bindings', () => ({ commands }))

const { useReportBundle, NO_LOG_FILES, BUNDLE_FAILED } = await import('./useReportBundle')
const { useToast } = await import('./useToast')

function lastToast() {
  const { toasts } = useToast()
  return toasts.value[toasts.value.length - 1]
}

describe('useReportBundle (0.x HelpPopover downloadBundle, spec section 8.1)', () => {
  beforeEach(() => {
    commands.saveReportBundle.mockReset()
    useToast().toasts.value = []
  })

  it('is busy while the bundle is written, then toasts the save', async () => {
    let finish: (value: unknown) => void = () => {}
    commands.saveReportBundle.mockReturnValue(new Promise((resolve) => (finish = resolve)))
    const bundle = useReportBundle()
    const running = bundle.download()
    expect(bundle.busy.value).toBe(true)
    finish({ status: 'ok', data: 'saved' })
    await running
    expect(bundle.busy.value).toBe(false)
    expect(bundle.error.value).toBeNull()
    expect(lastToast()).toMatchObject({ type: 'success', title: 'Saved', message: 'Report data saved' })
  })

  it('does nothing when the save dialog is cancelled', async () => {
    commands.saveReportBundle.mockResolvedValue({ status: 'ok', data: 'cancelled' })
    const bundle = useReportBundle()
    await bundle.download()
    expect(bundle.error.value).toBeNull()
    expect(lastToast()).toBeUndefined()
  })

  it('says no log files were found, with the settings link', async () => {
    commands.saveReportBundle.mockResolvedValue({ status: 'ok', data: 'noLogFiles' })
    const bundle = useReportBundle()
    await bundle.download()
    expect(bundle.error.value).toEqual({ message: NO_LOG_FILES, showSettingsLink: true })
    expect(NO_LOG_FILES).toBe("Couldn't find log files: check your log path in settings.")
  })

  it('says something went wrong on any other failure, without the link', async () => {
    commands.saveReportBundle.mockResolvedValue({ status: 'error', error: { kind: 'storage', message: 'disk full' } })
    const bundle = useReportBundle()
    await bundle.download()
    expect(bundle.error.value).toEqual({ message: BUNDLE_FAILED, showSettingsLink: false })
    expect(BUNDLE_FAILED).toBe('Something went wrong building the report bundle.')

    const errors = vi.spyOn(console, 'error').mockImplementation(() => {})
    commands.saveReportBundle.mockRejectedValue(new Error('ipc'))
    await bundle.download()
    expect(bundle.error.value).toEqual({ message: BUNDLE_FAILED, showSettingsLink: false })
    expect(bundle.busy.value).toBe(false)
    errors.mockRestore()
  })

  it('clears the last error when trying again', async () => {
    commands.saveReportBundle.mockResolvedValue({ status: 'ok', data: 'noLogFiles' })
    const bundle = useReportBundle()
    await bundle.download()
    commands.saveReportBundle.mockResolvedValue({ status: 'ok', data: 'saved' })
    await bundle.download()
    expect(bundle.error.value).toBeNull()
  })
})
