<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { SETTINGS_NAV, SETTINGS_NAV_HEADER, settingsPageFor } from '@/lib/settingsNav'

// 0.x `components/settings/SettingsSideNav.vue`.
const route = useRoute()
const currentPage = computed(() => settingsPageFor(route.path)?.key ?? null)
const { hasFeature } = usePlatformContext()
const items = computed(() => SETTINGS_NAV.filter((item) => item.feature === undefined || hasFeature(item.feature)))
</script>

<template>
  <div class="flex h-full flex-col border-r border-black/80 bg-muted/20">
    <div class="border-b border-black/80 px-3 py-3">
      <h2 class="text-sm font-semibold text-foreground">{{ SETTINGS_NAV_HEADER.title }}</h2>
      <p class="text-xs text-muted-foreground">{{ SETTINGS_NAV_HEADER.description }}</p>
    </div>
    <nav class="flex flex-1 flex-col gap-0.5 px-2 py-3">
      <RouterLink
        v-for="item in items"
        :key="item.key"
        :to="item.to"
        class="flex items-center gap-3 rounded px-3 py-2 text-sm font-medium transition-colors"
        :class="currentPage === item.key ? 'nav-item-active' : 'nav-item-inactive'"
      >
        <component
          :is="item.icon"
          class="size-4 shrink-0 transition-[color,filter] duration-150"
          :class="{ 'nav-icon-active': currentPage === item.key }"
        />
        <span>{{ item.label }}</span>
      </RouterLink>
    </nav>
  </div>
</template>
