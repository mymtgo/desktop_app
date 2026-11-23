import { describe, expect, it } from 'vitest'
import { channelOf, parseLicences } from './licences'

describe('the licence list', () => {
  it('reads the generated file', () => {
    const list = parseLicences(
      '{"packages":[{"name":"a","version":"1","licence":"MIT","ecosystem":"rust","texts":[0]}],"texts":["x"]}',
    )
    expect(list.packages[0].name).toBe('a')
    expect(list.texts).toEqual(['x'])
  })

  it('names the channel from the version until update channels land', () => {
    expect(channelOf('1.0.0-beta.2')).toBe('edge')
    expect(channelOf('1.0.0')).toBe('stable')
  })
})
