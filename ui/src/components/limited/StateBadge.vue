<script setup lang="ts">
import { computed } from 'vue'
import { Badge } from '@/components/ui/badge'
import { stateBadgeTint } from '@/lib/limited'

/**
 * A Limited run's state (0.x `StateBadge`): a tinted outline, as it sits in a
 * row of plain chips; a live run (`default`) gets a pulsing dot.
 */
const props = defineProps<{ label: string; variant: string }>()

const tint = computed(() => stateBadgeTint(props.variant))
const isLive = computed(() => props.variant === 'default')
</script>

<template>
  <Badge variant="outline" :class="tint">
    <span v-if="isLive" class="size-1.5 animate-pulse rounded-full bg-sky-400" />
    {{ label }}
  </Badge>
</template>
