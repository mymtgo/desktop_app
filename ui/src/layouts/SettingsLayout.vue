<script setup lang="ts">
import { computed } from 'vue'
import { RouterView, useRoute } from 'vue-router'
import SettingsSideNav from '@/components/settings/SettingsSideNav.vue'
import { settingsPageFor } from '@/lib/settingsNav'

// 0.x `layouts/SettingsLayout.vue`: the side nav, the page's heading and
// description, then the page's column of cards.
const route = useRoute()
const heading = computed(() => settingsPageFor(route.path))
// A route with `meta: { wide: true }` (Archives) uses the full width; the
// other pages stay a readable column.
const wide = computed(() => route.meta?.wide === true)
</script>

<template>
  <div class="flex min-h-0 flex-1">
    <div class="w-56 shrink-0">
      <SettingsSideNav />
    </div>
    <div class="flex min-h-0 flex-1 flex-col overflow-y-auto border-l border-white/5">
      <header v-if="heading" class="border-b border-black/60 bg-background/40 px-6 py-4">
        <h1 class="text-lg font-semibold text-foreground">{{ heading.title }}</h1>
        <p class="text-sm text-muted-foreground">{{ heading.description }}</p>
      </header>
      <div class="flex-1 overflow-auto">
        <div class="p-6" :class="{ 'max-w-3xl': !wide }">
          <RouterView />
        </div>
      </div>
    </div>
  </div>
</template>
