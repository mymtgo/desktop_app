import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { UpdateStatus } from '@/bindings'

const getUpdateStatus = vi.fn()
vi.mock('@/bindings', () => ({ commands: { getUpdateStatus } }))

const { fetchUpdateStatus } = await import('./queries')
const { updatePushes } = await import('./liveEvents')
const { queryClient } = await import('@/queryClient')
const { keys } = await import('./keys')

function status(phase: UpdateStatus['phase']): UpdateStatus {
  return { phase } as UpdateStatus
}

describe('fetchUpdateStatus', () => {
  beforeEach(() => {
    getUpdateStatus.mockReset()
    queryClient.clear()
  })

  it('keeps a pushed status that arrived while the fetch ran', async () => {
    getUpdateStatus.mockImplementation(async () => {
      // The "ready" push lands while the command is still answering.
      queryClient.setQueryData(keys.updates, status('ready'))
      updatePushes.note()
      return status('downloading')
    })
    expect((await fetchUpdateStatus(queryClient)).phase).toBe('ready')
  })

  it('returns the fetched status when nothing was pushed meanwhile', async () => {
    getUpdateStatus.mockResolvedValue(status('up_to_date'))
    expect((await fetchUpdateStatus(queryClient)).phase).toBe('up_to_date')
  })
})
