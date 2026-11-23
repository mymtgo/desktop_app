import { stripSecrets } from './strip'

export const CAPTURE_FORMAT_VERSION = 1

export interface Frame {
  at: number
  src: 'game_ws' | 'lobby_ws' | 'rest'
  dir: 'in' | 'out'
  url: string
  text: string
}

// Mirrors the Rust CaptureHeader (crates/endstep-capture/src/capture.rs).
export interface UnitHeader {
  mymtgo_endstep_capture: number
  unit_id: string
  kind: 'match' | 'session'
  started_at: number
  counter: number
  extension: string
  browser: string
  developer: boolean
  classification: 'human' | 'bot' | 'unknown'
}

export function frameLine(frame: Frame): string {
  const { src, dir, url, text } = frame
  const at = Math.trunc(frame.at)
  try {
    const body = stripSecrets(JSON.parse(text))
    return JSON.stringify({ at, src, dir, url, body })
  } catch {
    return JSON.stringify({ at, src, dir, url, text })
  }
}

export function headerLine(header: UnitHeader): string {
  return JSON.stringify(header)
}
