/**
 * The Node test environment with the browser's module transforms, for the
 * replay viewer's component tests (`*.client.test.ts`, the `client` project
 * in `vite.config.ts`). Vitest's plain `node` environment compiles `.vue`
 * files for server rendering only, so they cannot be mounted; this one
 * compiles them as the window does, with no DOM library added.
 *
 * The shape is Vitest's `Environment`, which its public types do not export.
 */
const clientTestEnvironment = {
  name: 'replay-client',
  viteEnvironment: 'client',
  setup() {
    return { teardown() {} }
  },
}

export default clientTestEnvironment
