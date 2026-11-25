<script setup lang="ts">
import {
  BookOpen,
  ChartPie,
  ChevronDown,
  FileUp,
  LayoutDashboard,
  Layers,
  Layers2,
  List,
  Settings,
  Sparkles,
  Swords,
  Trophy,
} from 'lucide-vue-next'
import { computed, type Component } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import icon from '@/assets/icon.png'
import { commands, type PlatformView } from '@/bindings'
import PlatformSwitch from '@/components/PlatformSwitch.vue'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { useToast } from '@/composables/useToast'
import { navItemActive } from '@/lib/appNav'
import { errorMessage, unwrap } from '@/lib/commands'
import type { Feature } from '@/lib/platformFeatures'
import { useAccounts, useAppInfo } from '@/lib/queries'
import { useWrapped } from '@/lib/wrapped/useWrapped'

const route = useRoute()
const router = useRouter()
const { add } = useToast()
const { data: accountsView } = useAccounts()
const { data: info } = useAppInfo()
// The "Edge" tag follows the version, not the product name (release spec 13.1).
const showEdgeTag = computed(() => info.value?.prerelease === true)

// 0.x AppHeader: a dropdown of the tracked accounts when there is more than
// one, else the current account's username (release spec 5).
const tracked = computed(() => (accountsView.value?.accounts ?? []).filter((account) => account.tracked))
const current = computed(() => accountsView.value?.current ?? null)

/** Switch, then open the Decks page (0.x redirects to the deck index). `AccountChanged` refetches every page. */
async function switchAccount(username: string) {
  try {
    const switched = unwrap(await commands.switchAccount(username))
    await router.push(switched.redirect)
  } catch (error) {
    add({ type: 'error', title: 'Could not switch account', message: errorMessage(error) })
  }
}

const { context, hasFeature, livePlatforms, switchTo } = usePlatformContext()

async function switchPlatform(next: PlatformView) {
  try {
    await switchTo(next)
  } catch (error) {
    add({ type: 'error', title: 'Could not switch platform', message: errorMessage(error) })
  }
}

const allNav: { label: string; icon: Component; to: string; feature?: Feature }[] = [
  { label: 'Dashboard', icon: LayoutDashboard, to: '/' },
  { label: 'Matches', icon: List, to: '/matches' },
  { label: 'Decks', icon: Layers, to: '/decks' },
  { label: 'Leagues', icon: Trophy, to: '/leagues', feature: 'leagues' },
  { label: 'Limited', icon: BookOpen, to: '/limited', feature: 'limited' },
  { label: 'Opponents', icon: Swords, to: '/opponents' },
  { label: 'Metagame', icon: ChartPie, to: '/metagame' },
  { label: 'Cards', icon: Layers2, to: '/cards', feature: 'cardsPage' },
]
// What the current platform has no data for is left out (arena spec section 15).
const nav = computed(() => allNav.filter((item) => item.feature === undefined || hasFeature(item.feature)))

// This year's Wrapped, after Cards while it is out (December).
const { bundle: wrapped } = useWrapped()
const wrappedLabel = computed(() => (wrapped.value ? `${wrapped.value.year} Wrapped` : null))

/** A section stays active on its sub-pages (a deck's pages, its matches included, sit under /decks). */
function isActive(to: string): boolean {
  return navItemActive(route.path, to)
}
</script>

<template>
  <header
    class="flex h-12 shrink-0 items-center justify-between gap-4 border-b border-black/80 bg-black/10 px-4 text-sidebar-foreground"
  >
    <div class="flex min-w-0 items-center gap-3">
      <RouterLink to="/" class="flex shrink-0 items-center gap-2" aria-label="MyMTGO home">
        <img :src="icon" alt="MyMTGO" class="size-7 rounded-md" />
        <span v-if="showEdgeTag" data-edge-tag class="font-['Bungee'] text-[11px] tracking-wide text-warning">Edge</span>
      </RouterLink>
      <PlatformSwitch v-if="context" :context="context" :live="livePlatforms" @switch="switchPlatform" />
      <nav class="flex shrink-0 items-center gap-1">
        <RouterLink
          v-for="item in nav"
          :key="item.label"
          :to="item.to"
          class="relative inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm font-medium transition-colors"
          :class="
            isActive(item.to)
              ? 'text-background-accent border-black shadow-inner shadow-black outline-[1px] outline-white/10'
              : 'bevel border-black/60 text-white hover:bg-accent/50 hover:text-accent-foreground'
          "
        >
          <component :is="item.icon" class="size-4" />
          {{ item.label }}
        </RouterLink>
        <RouterLink
          v-if="wrappedLabel"
          data-wrapped-link
          to="/wrapped"
          class="relative inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm font-medium transition-colors"
          :class="
            isActive('/wrapped')
              ? 'border-black text-white shadow-[inset_0_0_12px_-2px_rgb(56_189_248_/_35%)] outline-[1px] outline-white/10'
              : 'bevel border-black/60 text-white shadow-[inset_0_0_10px_-3px_rgb(56_189_248_/_30%)] hover:bg-accent/50'
          "
        >
          <Sparkles class="size-4 text-sky-300/80" />
          {{ wrappedLabel }}
        </RouterLink>
      </nav>
    </div>
    <div class="flex shrink-0 items-center gap-2">
      <DropdownMenu v-if="tracked.length > 1">
        <DropdownMenuTrigger
          class="inline-flex items-center gap-1 rounded-md px-2 py-1 text-sm text-sidebar-foreground/70 transition-colors hover:text-sidebar-foreground"
        >
          {{ current ?? 'No account' }}
          <ChevronDown class="size-3" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem
            v-for="account in tracked"
            :key="account.id"
            :class="{ 'font-semibold': account.active }"
            @click="switchAccount(account.username)"
          >
            {{ account.label }}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <span v-else-if="current" data-account class="text-sm text-sidebar-foreground/70">{{ current }}</span>
      <RouterLink
        v-if="hasFeature('import')"
        to="/import"
        class="bevel inline-flex items-center gap-1.5 rounded-md border border-black/60 px-3 py-1.5 text-sm font-medium text-white hover:bg-accent/50"
      >
        <FileUp class="size-4" />
        Import
      </RouterLink>
      <RouterLink
        to="/settings/general"
        class="bevel inline-flex items-center gap-1.5 rounded-md border border-black/60 px-3 py-1.5 text-sm font-medium text-white hover:bg-accent/50"
      >
        <Settings class="size-4" />
        Settings
      </RouterLink>
    </div>
  </header>
</template>
