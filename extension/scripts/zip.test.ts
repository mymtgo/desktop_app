import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { crc32, inflateRawSync } from 'node:zlib'
import { describe, expect, it } from 'vitest'
import { filesUnder, zipEntries } from './zip'

// Reads the entries back through the central directory.
function readZip(zip: Buffer): { name: string; data: Buffer; crc: number }[] {
  const end = zip.length - 22
  expect(zip.readUInt32LE(end)).toBe(0x06054b50)
  const count = zip.readUInt16LE(end + 10)
  let pos = zip.readUInt32LE(end + 16)
  const out: { name: string; data: Buffer; crc: number }[] = []
  for (let i = 0; i < count; i++) {
    const crc = zip.readUInt32LE(pos + 16)
    const packedSize = zip.readUInt32LE(pos + 20)
    const nameLength = zip.readUInt16LE(pos + 28)
    const localAt = zip.readUInt32LE(pos + 42)
    const name = zip.subarray(pos + 46, pos + 46 + nameLength).toString('utf8')
    const dataAt = localAt + 30 + zip.readUInt16LE(localAt + 26) + zip.readUInt16LE(localAt + 28)
    out.push({ name, data: inflateRawSync(zip.subarray(dataAt, dataAt + packedSize)), crc })
    pos += 46 + nameLength
  }
  return out
}

describe('zipEntries', () => {
  const entries = [
    { name: 'package.json', data: Buffer.from('{"version":"1.0.0"}') },
    { name: 'src/a.ts', data: Buffer.from('export const a = 1\n'.repeat(50)) },
  ]

  it('writes_every_entry_with_its_name_and_bytes', () => {
    const read = readZip(zipEntries(entries))
    expect(read.map((e) => e.name)).toEqual(['package.json', 'src/a.ts'])
    expect(read[1].data).toEqual(entries[1].data)
    expect(read[1].crc).toBe(crc32(entries[1].data))
  })

  it('gives_the_same_bytes_for_the_same_files', () => {
    expect(zipEntries(entries).equals(zipEntries(entries))).toBe(true)
  })
})

describe('filesUnder', () => {
  it('the_source_zip_skips_dotfiles_and_dot_folders', () => {
    const root = mkdtempSync(join(tmpdir(), 'mymtgo-files-under-'))
    mkdirSync(join(root, 'src', '.cache'), { recursive: true })
    writeFileSync(join(root, 'src', 'a.ts'), 'a')
    writeFileSync(join(root, 'src', '.DS_Store'), 'x')
    writeFileSync(join(root, 'src', '.a.ts.swp'), 'x')
    writeFileSync(join(root, 'src', '.cache', 'b.ts'), 'b')
    const names = filesUnder(join(root, 'src')).map((file) => file.slice(root.length + 1))
    expect(names).toEqual(['src/a.ts'])
  })
})
