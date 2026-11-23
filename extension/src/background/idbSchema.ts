// IndexedDB schema versions as pure data, so the upgrade path is testable without IndexedDB.
//
// Rule: a version only ever adds. Version N+1 gets a new entry in UPGRADE_STEPS that creates
// what it adds and never alters or deletes what an older version made, so a profile holding
// rows from any older version upgrades in place without losing them. Bump DB_VERSION and add
// the step in the same change. Never lower DB_VERSION: an older build cannot open a newer
// database (VersionError), so capture stops until the newer build returns. SchemaBuilder only
// creates stores; when the first step needs an index on an existing store, add a createIndex
// method to it rather than bypassing upgradeSchema.

// The part of an IDBDatabase an upgrade step may use.
export interface SchemaBuilder {
  createStore(name: string, options?: { keyPath?: string; autoIncrement?: boolean }, indexes?: string[]): void
}

export const DB_VERSION = 1

// Key N holds the step that takes a database from version N-1 to N.
export const UPGRADE_STEPS: Record<number, (schema: SchemaBuilder) => void> = {
  // Version 1: `units` (by unitId), `frames` (auto-increment key, index on unitId) and `meta`
  // (the per-profile capture counter).
  1: (schema) => {
    schema.createStore('units', { keyPath: 'unitId' })
    schema.createStore('frames', { autoIncrement: true }, ['unitId'])
    schema.createStore('meta')
  },
}

// Runs every step after `oldVersion` up to `newVersion`, in order (oldVersion 0 is a new database).
export function upgradeSchema(
  schema: SchemaBuilder,
  oldVersion: number,
  newVersion: number,
  steps: Record<number, (schema: SchemaBuilder) => void> = UPGRADE_STEPS,
): void {
  for (let version = oldVersion + 1; version <= newVersion; version++) {
    const step = steps[version]
    if (!step) throw new Error(`no IndexedDB upgrade step for version ${version}`)
    step(schema)
  }
}
