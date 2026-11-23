<script setup lang="ts">
import { ref } from 'vue'
import type { AutomaticLogStatus } from '@/lib/storage'

// One line for a log left on automatic: where it was found, or how many
// places were checked, with the list behind View locations.
defineProps<{ status: AutomaticLogStatus }>()

const expanded = ref(false)
</script>

<template>
  <div class="flex flex-col gap-1">
    <div class="flex items-center gap-2">
      <div
        class="size-2 shrink-0 rounded-full"
        :class="{
          'bg-primary': status.state === 'found',
          'bg-destructive': status.state === 'missing',
          'bg-muted-foreground': status.state === 'searching',
        }"
      />
      <span class="text-sm" :class="status.state === 'missing' ? 'text-destructive' : 'text-muted-foreground'">
        {{ status.message }}
      </span>
      <button
        v-if="status.state === 'missing' && status.searched.length > 0"
        type="button"
        class="text-xs text-muted-foreground underline"
        @click="expanded = !expanded"
      >
        {{ expanded ? 'Hide locations' : 'View locations' }}
      </button>
    </div>
    <code v-for="path in status.paths" :key="path" class="ml-4 break-all text-xs select-all">{{ path }}</code>
    <ul v-if="status.state === 'missing' && expanded" class="ml-4 space-y-0.5 text-xs text-muted-foreground">
      <li v-for="folder in status.searched" :key="folder">
        <code class="break-all select-all">{{ folder }}</code>
      </li>
    </ul>
  </div>
</template>
