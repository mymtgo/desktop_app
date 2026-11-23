import { defineComponent, h, type PropType } from 'vue'

/**
 * Test stand-ins for `vue-router`'s components, for component tests mounted
 * with `mountFake` (no router installed). A `RouterLink` renders an `a` whose
 * `href` is its `to`; a `RouterView` renders an empty `router-view` element.
 * Not used by the app.
 */
export const RouterLinkStub = defineComponent({
  name: 'RouterLink',
  props: { to: { type: [String, Object] as PropType<string | Record<string, unknown>>, required: true } },
  setup(props, { slots }) {
    return () => h('a', { href: props.to }, slots.default?.())
  },
})

export const RouterViewStub = defineComponent({
  name: 'RouterView',
  setup() {
    return () => h('router-view')
  },
})
