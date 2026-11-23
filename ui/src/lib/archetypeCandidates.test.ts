import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ArchetypeChoiceView, ArchetypeSourceView } from '@/bindings'

// T7 review I1: "Move to archetype" lists only the targets the reassign
// accepts; "Merge" keeps the merge candidates.
const commands = { mergeCandidates: vi.fn(), reassignCandidates: vi.fn() }
vi.mock('@/bindings', () => ({ commands }))

const { candidateLoader, filterCandidates } = await import('./archetypes')

const choice = (id: number, name: string, source: ArchetypeSourceView): ArchetypeChoiceView => ({
  id,
  name,
  format: 'modern',
  source,
})

describe('the dialogs candidate lists', () => {
  beforeEach(() => {
    commands.mergeCandidates.mockReset()
    commands.reassignCandidates.mockReset()
    commands.mergeCandidates.mockResolvedValue({
      status: 'ok',
      data: [choice(2, 'Qapi', 'downloaded'), choice(3, 'Qbrew', 'manual')],
    })
    commands.reassignCandidates.mockResolvedValue({ status: 'ok', data: [choice(3, 'Qbrew', 'manual')] })
  })

  it('the reassign dialog asks for the reassign targets only', async () => {
    expect(await candidateLoader('reassign')(7)).toEqual([choice(3, 'Qbrew', 'manual')])
    expect(commands.reassignCandidates).toHaveBeenCalledWith(7)
    expect(commands.mergeCandidates).not.toHaveBeenCalled()
  })

  it('the merge dialog keeps every merge candidate', async () => {
    expect((await candidateLoader('merge')(7)).map((candidate) => candidate.name)).toEqual(['Qapi', 'Qbrew'])
    expect(commands.reassignCandidates).not.toHaveBeenCalled()
  })

  it('filters by name, any case', () => {
    const list = [choice(2, 'Qapi Tron', 'downloaded'), choice(3, 'Qbrew', 'manual')]
    expect(filterCandidates(list, ' tron ')).toEqual([list[0]])
    expect(filterCandidates(list, '')).toEqual(list)
  })
})
