import { describe, expect, it } from 'vitest'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'
import SetByYouBadge from './SetByYouBadge.vue'

const tags = (userState: string | null) =>
  findAll(mountFake(SetByYouBadge, { userState }).root, (node) => node.props['data-testid'] === 'set-by-you')

describe('the Set by you badge', () => {
  it('names the status the player set', () => {
    const [tag] = tags('partial')
    expect(textOf(tag)).toBe('Set by you')
    expect(tag.props.title).toBe('Status set by you: Partial')
  })

  it('shows its own tooltip on a match the player fixed', () => {
    const [tag] = findAll(
      mountFake(SetByYouBadge, { tooltip: 'Result set by you' }).root,
      (node) => node.props['data-testid'] === 'set-by-you',
    )
    expect(textOf(tag)).toBe('Set by you')
    expect(tag.props.title).toBe('Result set by you')
  })

  it('shows nothing for Automatic', () => {
    expect(tags(null)).toEqual([])
  })
})
