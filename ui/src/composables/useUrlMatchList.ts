import { computed, watch, type Ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePlatformContext } from '@/composables/usePlatformContext'
import {
  matchListFromUrl,
  matchListQuery,
  matchListUrlQuery,
  nextSort,
  withListQuery,
  withoutHiddenSort,
  type MatchListState,
  type SortColumn,
} from '@/lib/matchList'
import { pageToShow } from '@/lib/paging'

/** What the step back needs of a shown match page. */
export interface ShownMatchPage {
  page: number
  rows: unknown[]
  total: number
  perPage: number
}

/**
 * The page to step back to when the page the URL asks for came back empty
 * (beyond the last), else `null`. A page still loading for an older URL is
 * left alone.
 */
export function emptiedPageTarget(shown: ShownMatchPage | null | undefined, listPage: number): number | null {
  if (!shown || shown.page !== listPage) return null
  return pageToShow({ page: shown.page, rows: shown.rows.length, total: shown.total, perPage: shown.perPage })
}

export interface UrlMatchListOptions {
  /**
   * Keep the URL's other keys (`?version=`, `?timeframe=`) beside the list's
   * own. On by default; the Matches page has none and writes only the list.
   */
  keepOtherQuery?: boolean
  /** The table shows one result column (Bo1): a sort by a hidden game column is unsorted. */
  singleResult?: Ref<boolean>
}

/**
 * A match list whose filters, sort and page live in the URL, so going back
 * from a match finds the list as it was.
 */
export function useUrlMatchList(options: UrlMatchListOptions = {}) {
  const keepOtherQuery = options.keepOtherQuery ?? true
  const route = useRoute()
  const router = useRouter()
  const { platform } = usePlatformContext()

  const list = computed(() =>
    withoutHiddenSort(matchListFromUrl(route.query, platform.value), options.singleResult?.value ?? false),
  )
  const query = computed(() => matchListQuery(list.value))

  function show(next: MatchListState, replace = false) {
    const location = { query: keepOtherQuery ? withListQuery(route.query, next) : matchListUrlQuery(next) }
    void (replace ? router.replace(location) : router.push(location))
  }

  function sort(column: SortColumn) {
    show({ ...list.value, ...nextSort(list.value, column), page: 1 })
  }

  /** Removals or a refresh can empty the page being viewed: step back instead of showing an empty table. */
  function stepBackFromEmptyPage(shown: Ref<ShownMatchPage | null | undefined>) {
    watch(shown, (page) => {
      const target = emptiedPageTarget(page, list.value.page)
      if (target !== null) show({ ...list.value, page: target }, true)
    })
  }

  return { list, query, show, sort, stepBackFromEmptyPage }
}
