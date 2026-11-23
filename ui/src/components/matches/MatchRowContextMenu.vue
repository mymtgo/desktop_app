<script setup lang="ts">
import { Layers, NotepadText, Pencil, RefreshCw, Tags, Trash2, Trophy, X } from 'lucide-vue-next'
import {
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuLabel,
  ContextMenuPortal,
  ContextMenuRoot,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from 'reka-ui'

/**
 * A match row's right-click menu (0.x `MatchRowContextMenu`): "Edit
 * match..." for a stored match (issue #393), notes, the
 * archetype items (set by hand, clear, and "Detect" where the page wires
 * detection), "Set deck..." (issue #332), "Move to league..." for a finished match and
 * "Delete match" (league self-management spec 5.4). A draft seat has no
 * archetype and a draft match no constructed deck, so a Limited list leaves
 * the archetype and deck items out.
 */
withDefaults(
  defineProps<{
    notes: string | null
    /** Show "Edit match..." (the row is stored, so it has an id). */
    editable?: boolean
    /** Show the archetype items. */
    archetypes?: boolean
    /** The opponent has an archetype ("Clear" is enabled). */
    hasArchetype?: boolean
    /** The page detects archetypes ("Detect" shows). */
    detectable?: boolean
    /** Show "Set deck...". */
    deck?: boolean
    /** Show "Move to league..." (a complete or abandoned match). */
    movable?: boolean
    /** No menu (a match still in progress is not stored, so its edits are refused). */
    disabled?: boolean
  }>(),
  { editable: false, archetypes: false, hasArchetype: false, detectable: false, deck: false, movable: false, disabled: false },
)

const emit = defineEmits<{
  edit: []
  openNotes: []
  remove: []
  setArchetype: []
  clearArchetype: []
  detect: []
  setDeck: []
  moveToLeague: []
}>()

const itemClass =
  'focus:bg-accent focus:text-accent-foreground relative flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*=\'size-\'])]:size-4'
</script>

<template>
  <ContextMenuRoot>
    <ContextMenuTrigger as-child :disabled="disabled">
      <slot />
    </ContextMenuTrigger>
    <ContextMenuPortal>
      <ContextMenuContent
        class="z-50 w-56 min-w-[8rem] overflow-hidden rounded-md border bg-popover p-1 text-popover-foreground shadow-md data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:animate-in data-[state=open]:fade-in-0"
      >
        <ContextMenuItem v-if="editable" :class="itemClass" @select="emit('edit')">
          <Pencil class="text-muted-foreground" />
          Edit match...
        </ContextMenuItem>
        <ContextMenuItem :class="itemClass" @select="emit('openNotes')">
          <NotepadText class="text-muted-foreground" />
          {{ notes ? 'Edit notes' : 'Add notes' }}
        </ContextMenuItem>
        <ContextMenuSeparator class="-mx-1 my-1 h-px bg-border" />
        <template v-if="archetypes">
          <ContextMenuLabel class="px-2 py-1.5 text-xs tracking-wide text-muted-foreground uppercase">
            Archetype
          </ContextMenuLabel>
          <ContextMenuItem v-if="detectable" :class="itemClass" @select="emit('detect')">
            <RefreshCw class="text-muted-foreground" />
            Detect
          </ContextMenuItem>
          <ContextMenuItem :class="itemClass" @select="emit('setArchetype')">
            <Tags class="text-muted-foreground" />
            Set manually
          </ContextMenuItem>
          <ContextMenuItem :class="itemClass" :disabled="!hasArchetype" @select="emit('clearArchetype')">
            <X class="text-muted-foreground" />
            Clear
          </ContextMenuItem>
          <ContextMenuSeparator class="-mx-1 my-1 h-px bg-border" />
        </template>
        <template v-if="deck">
          <ContextMenuLabel class="px-2 py-1.5 text-xs tracking-wide text-muted-foreground uppercase">
            Deck
          </ContextMenuLabel>
          <ContextMenuItem :class="itemClass" @select="emit('setDeck')">
            <Layers class="text-muted-foreground" />
            Set deck...
          </ContextMenuItem>
          <ContextMenuSeparator class="-mx-1 my-1 h-px bg-border" />
        </template>
        <template v-if="movable">
          <ContextMenuItem :class="itemClass" @select="emit('moveToLeague')">
            <Trophy class="text-muted-foreground" />
            Move to league...
          </ContextMenuItem>
          <ContextMenuSeparator class="-mx-1 my-1 h-px bg-border" />
        </template>
        <ContextMenuItem
          :class="[itemClass, 'text-destructive focus:bg-destructive/10 focus:text-destructive']"
          @select="emit('remove')"
        >
          <Trash2 />
          Delete match
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenuPortal>
  </ContextMenuRoot>
</template>
