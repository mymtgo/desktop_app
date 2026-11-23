import { describe, expect, it } from 'vitest'
import { DB_VERSION, UPGRADE_STEPS, upgradeSchema, type SchemaBuilder } from './idbSchema'

function fake() {
  const created: string[] = []
  const schema: SchemaBuilder = { createStore: (name) => void created.push(name) }
  return { created, schema }
}

describe('upgradeSchema', () => {
  it('a_new_database_gets_every_store', () => {
    const { created, schema } = fake()
    upgradeSchema(schema, 0, DB_VERSION)
    expect(created).toEqual(['units', 'frames', 'meta'])
  })

  it('an_up_to_date_database_runs_no_step', () => {
    const { created, schema } = fake()
    upgradeSchema(schema, DB_VERSION, DB_VERSION)
    expect(created).toEqual([])
  })

  it('only_the_steps_after_the_old_version_run_in_order', () => {
    const ran: number[] = []
    const steps = { 1: () => ran.push(1), 2: () => ran.push(2), 3: () => ran.push(3) }
    upgradeSchema(fake().schema, 1, 3, steps)
    expect(ran).toEqual([2, 3])
  })

  it('a_missing_step_is_an_error', () => {
    expect(() => upgradeSchema(fake().schema, 0, 2, { 1: () => {} })).toThrow(/version 2/)
  })

  it('every_version_up_to_the_current_has_a_step', () => {
    for (let v = 1; v <= DB_VERSION; v++) expect(UPGRADE_STEPS[v]).toBeTypeOf('function')
  })
})
