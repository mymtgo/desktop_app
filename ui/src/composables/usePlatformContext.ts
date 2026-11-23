import { computed, watch } from 'vue'
import { useRouter } from 'vue-router'
import { commands, type PlatformView } from '@/bindings'
import { unwrap } from '@/lib/commands'
import { keys } from '@/lib/keys'
import { livePlatforms } from '@/lib/liveEvents'
import { platformsWithLiveMatch } from '@/lib/platform'
import { hasFeature, routeAllowed, type Feature } from '@/lib/platformFeatures'
import { usePlatformContextQuery } from '@/lib/queries'
import { queryClient } from '@/queryClient'

/**
 * The platform the pages read (arena spec section 15): the context, the
 * feature map for it, and the switch. Until the context loads the app is MTGO.
 */
export function usePlatformContext() {
  const { data: context } = usePlatformContextQuery()
  const platform = computed<PlatformView>(() => context.value?.current ?? 'mtgo')
  const live = computed(() => platformsWithLiveMatch(livePlatforms.value))
  const router = useRouter()

  /** Whether the current platform has a feature (`platformFeatures.ts`). */
  function has(feature: Feature): boolean {
    return hasFeature(platform.value, feature)
  }

  /**
   * Switch, then open the redirect (the dashboard). The context is refetched
   * first so the router guard reads the new platform; `AccountChanged`
   * refetches every page.
   */
  async function switchTo(next: PlatformView): Promise<void> {
    const switched = unwrap(await commands.switchPlatform(next))
    await queryClient.refetchQueries({ queryKey: keys.platformContext })
    await router.push(switched.redirect)
  }

  return { context, platform, livePlatforms: live, hasFeature: has, switchTo }
}

/**
 * Leave a page the platform context hides once the context changes under
 * it (a switch, or a refetch after an account change): it goes to the
 * dashboard, as the router guard sends a new navigation. The main window
 * calls this once.
 */
export function useLeaveHiddenPages(): void {
  const { data: context } = usePlatformContextQuery()
  const router = useRouter()
  watch(
    () => context.value?.current,
    (platform) => {
      if (platform !== undefined && !routeAllowed(router.currentRoute.value.name, platform)) {
        void router.replace({ name: 'dashboard' })
      }
    },
  )
}
