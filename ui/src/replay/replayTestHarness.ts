import { createRenderer, createSSRApp, h, nextTick, shallowReactive, type Component } from 'vue'
import { renderToString } from 'vue/server-renderer'

/**
 * Test helpers for the replay viewer's components (vitest runs in Node with
 * no DOM). `renderHtml` renders a component to a string through Vue's own
 * server renderer; `mountFake` mounts it with a tiny renderer whose nodes are
 * plain objects, so `onMounted` runs and event handlers can be called.
 * Neither is used by the app.
 * Components are imported before `window` and `document` are stubbed, so
 * `@vueuse/core` sees no client: `useElementSize` reports 0, `ResizeObserver`
 * paths are skipped, and fake nodes have no `classList` or `addEventListener`;
 * compact layouts, scrubber geometry, transitions and zone window drags are
 * not exercised here.
 */

/** The bounds every fake element reports: a 1200 x 800 window at the origin. */
export const FAKE_BOUNDS = { left: 0, top: 0, right: 1200, bottom: 800, width: 1200, height: 800, x: 0, y: 0 }

export type FakeNode = {
  tag: string
  text: string
  props: Record<string, unknown>
  children: FakeNode[]
  parent: FakeNode | null
  style: Record<string, string>
  clientHeight: number
  offsetHeight: number
  getBoundingClientRect: () => typeof FAKE_BOUNDS
  querySelector: () => null
  querySelectorAll: () => FakeNode[]
  setPointerCapture: () => void
  releasePointerCapture: () => void
  scrollTo: () => void
  focus: () => void
}

/** Every node whose `focus()` was called since the last mount, oldest first; cleared on mount and unmount. */
export const focusLog: FakeNode[] = []

function fakeNode(tag: string, text = ''): FakeNode {
  const node: FakeNode = {
    tag,
    text,
    props: {},
    children: [],
    parent: null,
    style: {},
    clientHeight: 0,
    offsetHeight: 0,
    getBoundingClientRect: () => FAKE_BOUNDS,
    querySelector: () => null,
    querySelectorAll: () => [],
    setPointerCapture: () => {},
    releasePointerCapture: () => {},
    scrollTo: () => {},
    focus: () => {
      focusLog.push(node)
    },
  }
  return node
}

function detach(child: FakeNode) {
  const parent = child.parent
  if (!parent) return
  const index = parent.children.indexOf(child)
  if (index >= 0) parent.children.splice(index, 1)
  child.parent = null
}

const fakeRenderer = createRenderer<FakeNode, FakeNode>({
  createElement: (tag) => fakeNode(tag),
  createText: (text) => fakeNode('#text', text),
  createComment: (text) => fakeNode('#comment', text),
  setText: (node, text) => {
    node.text = text
  },
  setElementText: (node, text) => {
    node.children.forEach((child) => (child.parent = null))
    node.children = []
    if (text) {
      const child = fakeNode('#text', text)
      child.parent = node
      node.children.push(child)
    }
  },
  insert: (child, parent, anchor) => {
    detach(child)
    const at = anchor ? parent.children.indexOf(anchor) : -1
    if (at >= 0) parent.children.splice(at, 0, child)
    else parent.children.push(child)
    child.parent = parent
  },
  remove: (child) => detach(child),
  parentNode: (node) => node.parent,
  nextSibling: (node) => {
    const siblings = node.parent?.children ?? []
    return siblings[siblings.indexOf(node) + 1] ?? null
  },
  patchProp: (node, key, _previous, next) => {
    node.props[key] = next
  },
})

/** Every element under `node` (itself included) that `test` accepts, in tree order. */
export function findAll(node: FakeNode, test: (node: FakeNode) => boolean): FakeNode[] {
  const found = test(node) ? [node] : []
  for (const child of node.children) found.push(...findAll(child, test))
  return found
}

/** The text under a node, comments left out. */
export function textOf(node: FakeNode): string {
  if (node.tag === '#text') return node.text
  if (node.tag === '#comment') return ''
  return node.children.map(textOf).join('')
}

/** Whether a node's class list (string, array or object binding) holds `name`. */
export function hasClass(node: FakeNode, name: string): boolean {
  return classText(node.props.class).split(/\s+/).includes(name)
}

function classText(value: unknown): string {
  if (typeof value === 'string') return value
  if (Array.isArray(value)) return value.map(classText).join(' ')
  if (value && typeof value === 'object') {
    return Object.entries(value)
      .filter(([, on]) => on)
      .map(([name]) => name)
      .join(' ')
  }
  return ''
}

export type Mounted = {
  root: FakeNode
  /** Every Vue warning and error raised while mounted. */
  warnings: string[]
  unmount: () => void
  /** Assigns new props into the mounted component (re-renders on the next tick). */
  setProps: (next: Record<string, unknown>) => void
  /** Lets watchers, promises and re-renders settle. */
  settle: () => Promise<void>
}

/**
 * Mounts a component with the fake renderer. Props are held shallowly, as a
 * parent passing plain objects holds them: `setProps` replaces a prop, and
 * the component receives the object itself, not a deep reactive proxy.
 */
export function mountFake(component: Component, props: Record<string, unknown> = {}): Mounted {
  const warnings: string[] = []
  const root = fakeNode('root')
  const live = shallowReactive({ ...props })
  const app = fakeRenderer.createApp({ render: () => h(component, { ...live }) })
  app.config.warnHandler = (message) => {
    warnings.push(message)
  }
  app.config.errorHandler = (error) => {
    warnings.push(String(error))
  }
  focusLog.length = 0
  app.mount(root)
  return {
    root,
    warnings,
    setProps: (next) => {
      Object.assign(live, next)
    },
    unmount: () => {
      app.unmount()
      focusLog.length = 0
    },
    settle: async () => {
      for (let i = 0; i < 5; i++) {
        await Promise.resolve()
        await nextTick()
      }
    },
  }
}

/** Renders a component to HTML with Vue's server renderer, collecting warnings. */
export async function renderHtml(
  component: Component,
  props: Record<string, unknown> = {},
): Promise<{ html: string; warnings: string[] }> {
  const warnings: string[] = []
  const app = createSSRApp({ render: () => h(component, props) })
  app.config.warnHandler = (message) => {
    warnings.push(message)
  }
  app.config.errorHandler = (error) => {
    warnings.push(String(error))
  }
  const html = await renderToString(app)
  return { html, warnings }
}

/** The text of rendered HTML: tags and comments removed, spaces collapsed. */
export function htmlText(html: string): string {
  return html
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/<[^>]+>/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}
