import { getCurrentWindow } from '@tauri-apps/api/window'
import { computed, onUnmounted, ref, watch, watchEffect } from 'vue'
import { useRoute } from 'vue-router'
import { pageTitle } from '@/lib/pageTitle'
import { useAppInfo } from '@/lib/queries'

/** What the page on screen loaded and wants in front of its name (a deck or Limited event name). */
const detail = ref<string | null>(null)

/** A layout or page names what it shows; the title picks it up (see `pageTitle`). */
export function usePageDetail(name: () => string | null | undefined) {
  let mine: string | null = null
  watchEffect(() => {
    mine = name() ?? null
    detail.value = mine
  })
  onUnmounted(() => {
    // Another page may already have set its own.
    if (detail.value === mine) detail.value = null
  })
}

/** Keeps the window and document title on the current page. Called once, from the app root. */
export function useWindowTitle() {
  const route = useRoute()
  const { data: info } = useAppInfo()
  // The product name is read at runtime (release spec 13.1).
  const appName = computed(() => info.value?.productName ?? 'MyMTGO')
  const title = computed(() => pageTitle(route.name ? String(route.name) : null, detail.value, appName.value))
  watch(
    title,
    (next) => {
      document.title = next
      try {
        getCurrentWindow()
          .setTitle(next)
          .catch(() => {})
      } catch {
        // Not inside the app window (a browser): the document title is enough.
      }
    },
    { immediate: true },
  )
}
