<script setup lang="ts">
import { ArrowUpRight } from 'lucide-vue-next'
import { RouterLink, type RouteLocationRaw } from 'vue-router'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'

/**
 * One home dashboard card (dashboard redesign spec section 3): a sentence
 * case title, an optional aside (numbers, a switch), an optional link to
 * the full page, then the content at the card's own padding.
 */
defineProps<{ title: string; to?: RouteLocationRaw; linkLabel?: string }>()
</script>

<template>
  <Card class="gap-4">
    <CardHeader class="flex items-center justify-between gap-2">
      <CardTitle class="text-sm font-medium">{{ title }}</CardTitle>
      <div class="flex min-w-0 items-center gap-3 text-xs text-muted-foreground">
        <slot name="aside" />
        <RouterLink
          v-if="to"
          :to="to"
          :aria-label="linkLabel ?? `Open ${title.toLowerCase()}`"
          class="rounded p-0.5 hover:text-foreground"
        >
          <ArrowUpRight class="size-4" />
        </RouterLink>
      </div>
    </CardHeader>
    <CardContent>
      <slot />
    </CardContent>
  </Card>
</template>
