<script setup lang="ts">
import { ArrowRight, Sparkles, X } from 'lucide-vue-next'
import { RouterLink } from 'vue-router'
import type { WrappedBundle } from '@/bindings'
import { buttonVariants } from '@/components/ui/button'
import { Card } from '@/components/ui/card'

/**
 * The dashboard's way into Wrapped (Wrapped spec, mockup section 1): shown
 * above Results while this year's Wrapped is out and not dismissed. It is a
 * dashboard card like the others, one row high, and gives nothing
 * of the year away before it is opened.
 */
defineProps<{ bundle: WrappedBundle }>()
const emit = defineEmits<{ dismiss: [] }>()
</script>

<template>
  <Card class="flex-row items-center gap-3 px-4 py-3">
    <span class="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/15 text-primary">
      <Sparkles class="size-4" />
    </span>
    <p class="min-w-0 flex-1 truncate text-sm font-medium">Your {{ bundle.year }} in Magic is ready</p>
    <RouterLink :to="{ name: 'wrapped' }" :class="buttonVariants({ variant: 'outline', size: 'sm' })">
      Open Wrapped <ArrowRight class="size-3.5" />
    </RouterLink>
    <button
      type="button"
      class="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
      aria-label="Dismiss Wrapped"
      @click="emit('dismiss')"
    >
      <X class="size-4" />
    </button>
  </Card>
</template>
