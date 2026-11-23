import { readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { crc32, deflateRawSync } from 'node:zlib'

// A small zip writer: deflated entries, fixed timestamp, so the same files give the same bytes.
// An entry is a name and its bytes. Names use "/" and are written in the order given.
const DOS_TIME = 0
const DOS_DATE = (2020 - 1980) * 512 + 1 * 32 + 1

export interface ZipEntry {
  name: string
  data: Buffer
}

export function zipEntries(entries: ZipEntry[]): Buffer {
  const chunks: Buffer[] = []
  const central: Buffer[] = []
  let offset = 0
  for (const entry of entries) {
    const name = Buffer.from(entry.name, 'utf8')
    const packed = deflateRawSync(entry.data)
    const crc = crc32(entry.data)
    const local = Buffer.alloc(30)
    local.writeUInt32LE(0x04034b50, 0)
    local.writeUInt16LE(20, 4)
    local.writeUInt16LE(0x0800, 6)
    local.writeUInt16LE(8, 8)
    local.writeUInt16LE(DOS_TIME, 10)
    local.writeUInt16LE(DOS_DATE, 12)
    local.writeUInt32LE(crc, 14)
    local.writeUInt32LE(packed.length, 18)
    local.writeUInt32LE(entry.data.length, 22)
    local.writeUInt16LE(name.length, 26)
    chunks.push(local, name, packed)

    const head = Buffer.alloc(46)
    head.writeUInt32LE(0x02014b50, 0)
    head.writeUInt16LE(20, 4)
    head.writeUInt16LE(20, 6)
    head.writeUInt16LE(0x0800, 8)
    head.writeUInt16LE(8, 10)
    head.writeUInt16LE(DOS_TIME, 12)
    head.writeUInt16LE(DOS_DATE, 14)
    head.writeUInt32LE(crc, 16)
    head.writeUInt32LE(packed.length, 20)
    head.writeUInt32LE(entry.data.length, 24)
    head.writeUInt16LE(name.length, 28)
    head.writeUInt32LE(offset, 42)
    central.push(head, name)
    offset += local.length + name.length + packed.length
  }
  const centralSize = central.reduce((sum, part) => sum + part.length, 0)
  const end = Buffer.alloc(22)
  end.writeUInt32LE(0x06054b50, 0)
  end.writeUInt16LE(entries.length, 8)
  end.writeUInt16LE(entries.length, 10)
  end.writeUInt32LE(centralSize, 12)
  end.writeUInt32LE(offset, 16)
  return Buffer.concat([...chunks, ...central, end])
}

// Every file under `path`, sorted. Files and folders whose name starts with a dot are skipped
// (.DS_Store, editor swap files), so they never reach the source zip.
export function filesUnder(path: string): string[] {
  if (!statSync(path).isDirectory()) return [path]
  return readdirSync(path)
    .filter((name) => !name.startsWith('.'))
    .sort()
    .flatMap((name) => filesUnder(join(path, name)))
}
