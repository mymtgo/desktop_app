<script setup lang="ts">
import { ChevronDown, ChevronRight } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { licenceList } from '@/content/licenceList'
import type { LicencePackage } from '@/lib/licences'

// Every third-party package compiled into the app (release spec section 9),
// from the generated `content/licences.json`. A row expands to its texts.
const sections = computed(() => [
  { title: 'Rust crates', packages: licenceList.packages.filter((p) => p.ecosystem === 'rust') },
  { title: 'Interface packages', packages: licenceList.packages.filter((p) => p.ecosystem === 'ui') },
])

const expanded = ref(new Set<string>())

function keyOf(item: LicencePackage): string {
  return `${item.ecosystem}:${item.name}@${item.version}`
}

function toggle(item: LicencePackage) {
  const next = new Set(expanded.value)
  const key = keyOf(item)
  if (next.has(key)) next.delete(key)
  else next.add(key)
  expanded.value = next
}

function textsOf(item: LicencePackage): string[] {
  return item.texts.map((index) => licenceList.texts[index] ?? '')
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="space-y-1">
      <RouterLink to="/settings/advanced" class="text-xs text-muted-foreground hover:text-foreground">
        Back to Advanced
      </RouterLink>
      <h1 class="text-lg font-semibold">Third-party licences</h1>
      <p class="text-xs text-muted-foreground">
        MyMTGO is built with these packages. Select one to read its licence.
      </p>
    </div>

    <section v-for="section in sections" :key="section.title" class="rounded-lg border bg-card">
      <h2 class="border-b px-4 py-2 text-sm font-semibold">{{ section.title }} ({{ section.packages.length }})</h2>
      <ul class="divide-y">
        <li v-for="item in section.packages" :key="keyOf(item)">
          <button
            type="button"
            class="flex w-full items-center gap-2 px-4 py-2 text-left text-sm hover:bg-muted/40"
            :aria-expanded="expanded.has(keyOf(item))"
            @click="toggle(item)"
          >
            <ChevronDown v-if="expanded.has(keyOf(item))" class="size-4 shrink-0" />
            <ChevronRight v-else class="size-4 shrink-0" />
            <span class="font-medium">{{ item.name }}</span>
            <span class="text-muted-foreground">{{ item.version }}</span>
            <span class="ml-auto text-xs text-muted-foreground">{{ item.licence }}</span>
          </button>
          <div v-if="expanded.has(keyOf(item))" class="space-y-3 px-4 pb-3">
            <p v-if="item.texts.length === 0" class="text-xs text-muted-foreground">
              This package includes no licence file. Its licence is {{ item.licence }}.
            </p>
            <pre
              v-for="(text, index) in textsOf(item)"
              :key="index"
              class="max-h-96 overflow-auto rounded bg-muted/30 p-3 text-xs whitespace-pre-wrap select-text"
              >{{ text }}</pre
            >
          </div>
        </li>
      </ul>
    </section>
  </div>
</template>
