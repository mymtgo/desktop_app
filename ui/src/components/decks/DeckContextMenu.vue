<script setup lang="ts">
import { Tags, X } from 'lucide-vue-next'
import {
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuLabel,
  ContextMenuPortal,
  ContextMenuRoot,
  ContextMenuTrigger,
} from 'reka-ui'

/** A deck card's right-click menu: set the deck's archetype by hand, or clear it. */
defineProps<{
  /** The deck has an archetype ("Clear" is enabled). */
  hasArchetype: boolean
  /** A commander format deck: its archetype is its commander's and cannot be changed. */
  locked?: boolean
}>()

const emit = defineEmits<{
  setArchetype: []
  clearArchetype: []
}>()

const itemClass =
  'focus:bg-accent focus:text-accent-foreground relative flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*=\'size-\'])]:size-4'
</script>

<template>
  <ContextMenuRoot>
    <ContextMenuTrigger as-child>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuPortal>
      <ContextMenuContent
        class="z-50 w-56 min-w-[8rem] overflow-hidden rounded-md border bg-popover p-1 text-popover-foreground shadow-md data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:animate-in data-[state=open]:fade-in-0"
      >
        <ContextMenuLabel class="px-2 py-1.5 text-xs tracking-wide text-muted-foreground uppercase">
          Archetype
        </ContextMenuLabel>
        <ContextMenuItem :class="itemClass" :disabled="locked" @select="emit('setArchetype')">
          <Tags class="text-muted-foreground" />
          Set manually...
        </ContextMenuItem>
        <ContextMenuItem :class="itemClass" :disabled="!hasArchetype || locked" @select="emit('clearArchetype')">
          <X class="text-muted-foreground" />
          Clear
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenuPortal>
  </ContextMenuRoot>
</template>
