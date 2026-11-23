import type { RecorderStatus } from '@/bindings'

// The Replays settings page's status line (spec 2026-11-09 section 4), one
// text per `RecorderStatus` variant.

export const UNSUPPORTED_TEXT = 'Replays are recorded on Windows only'
export const NO_BUILD_TEXT = 'No recorder build in this version yet'

export function recorderStatusText(status: RecorderStatus): string {
  if (typeof status === 'object') return `Failed: ${status.failed.reason}`
  switch (status) {
    case 'off':
      return 'Off'
    case 'unsupported':
      return UNSUPPORTED_TEXT
    case 'noBuild':
      return NO_BUILD_TEXT
    case 'downloading':
      return 'Downloading the recorder'
    case 'blockedByAntivirus':
      return 'Blocked by antivirus: allow mtgr-record in Windows Security, then Retry'
    case 'waiting':
      return 'Waiting for MTGO'
    case 'ready':
      return 'Connected to MTGO, waiting for a match'
    case 'recording':
      return 'Recording'
    case 'anotherRunning':
      return 'Another recorder is running'
    case 'mtgoNotSupported':
      return 'This MTGO version is not supported yet; retrying every minute'
    case 'stoppedAfterCrashes':
      return 'Stopped after repeated crashes'
  }
}

/** The button under the status line, if the status has one. */
export function recorderStatusAction(status: RecorderStatus): string | null {
  if (typeof status === 'object' || status === 'blockedByAntivirus' || status === 'mtgoNotSupported') return 'Retry'
  return status === 'stoppedAfterCrashes' ? 'Start again' : null
}

/** The switch shows on for every status except off, unsupported and noBuild. */
export function recorderSwitchOn(status: RecorderStatus): boolean {
  return status !== 'off' && status !== 'unsupported' && status !== 'noBuild'
}

/** A recorder release file (NOTICE, THIRD-PARTY-NOTICES.md) for `version`. */
export function recorderFileUrl(version: string, file: string): string {
  return `https://releases.mymtgo.com/mtgr-record/${version}/${file}`
}
