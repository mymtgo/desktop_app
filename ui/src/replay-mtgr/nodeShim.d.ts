// The UI has no @types/node (no new packages). Only what goldens.test.ts uses from Node.
declare module 'node:fs' {
  export function existsSync(path: string): boolean
  export function readdirSync(path: string): string[]
  export function readFileSync(path: string, encoding: 'utf8'): string
}
declare module 'node:url' {
  export function fileURLToPath(url: URL): string
}
declare module 'node:process' {
  const process: { env: Record<string, string | undefined> }
  export default process
}
