// vite.config.ts runs in Node, but the UI does not depend on @types/node.
// This declares the one Node function the config uses so the file can be
// type-checked (`pnpm run check`) without adding a dependency.
declare module 'node:url' {
  export function fileURLToPath(url: string | URL): string
}
