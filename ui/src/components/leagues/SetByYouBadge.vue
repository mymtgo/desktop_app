<script setup lang="ts">
import { computed } from 'vue'
import type { RunStateView } from '@/bindings'
import { Badge } from '@/components/ui/badge'
import { statusLabel } from '@/lib/leagues'

/**
 * The "Set by you" tag a run card or Limited event row shows when the player
 * set the run's status (league self-management spec 3.2); nothing for
 * Automatic (`userState` null). Elsewhere (a match the player fixed, edit a
 * match spec section 3) the tag shows with its own `tooltip`.
 */
const props = withDefaults(defineProps<{ userState?: RunStateView | null; tooltip?: string | null }>(), {
  userState: null,
  tooltip: null,
})

const title = computed(
  () => props.tooltip ?? (props.userState ? `Status set by you: ${statusLabel(props.userState)}` : null),
)
</script>

<template>
  <Badge
    v-if="title"
    variant="outline"
    data-testid="set-by-you"
    class="shrink-0 text-[10px] tracking-wider text-muted-foreground uppercase"
    :title="title"
  >Set by you</Badge>
</template>
