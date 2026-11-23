<script setup lang="ts">
import { computed, watch } from 'vue'
import { useRouter } from 'vue-router'
import { entryFor } from '@/content/whats-new'
import { useAppInfo } from '@/lib/queries'

// 0.x `pages/whats-new/Show.vue` (release spec section 7): the newest entry
// not newer than the running version. Without one the page goes to the
// dashboard (0.x answered 404).
const router = useRouter()
const { data: info } = useAppInfo()

const entry = computed(() => (info.value ? entryFor(info.value.version) : null))

watch(
  () => info.value?.version,
  (version) => {
    if (version && entryFor(version) === null) void router.replace({ name: 'dashboard' })
  },
  { immediate: true },
)
</script>

<template>
  <div class="mx-auto flex w-full max-w-3xl flex-col px-6 py-8">
    <article v-if="entry" class="markdown-content">
      <component :is="entry.component" />
    </article>
  </div>
</template>
