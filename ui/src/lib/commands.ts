import type { CommandError } from '@/bindings'

/** A command returned an error. `error.kind` says which part failed. */
export class CommandFailed extends Error {
  constructor(public error: CommandError) {
    super(error.message)
  }
}

/** What every generated command resolves to. */
export type CommandResult<T> = { status: 'ok'; data: T } | { status: 'error'; error: CommandError }

/** The data of a command result, or a thrown `CommandFailed`. */
export function unwrap<T>(result: CommandResult<T>): T {
  if (result.status === 'ok') {
    return result.data
  }
  throw new CommandFailed(result.error)
}

export function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message
  }
  return String(error)
}
