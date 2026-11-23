<script setup lang="ts">
import { Camera, Check, Copy, ListPlus, MoreHorizontal, PencilLine, Tag, Trash2 } from 'lucide-vue-next'
import type { RunStateView } from '@/bindings'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { automaticLabel, STATUS_CHOICES } from '@/lib/leagues'

/**
 * A run's actions (0.x `LeagueContextMenu`, league self-management spec
 * 5.1): copy, notes, "Add matches...", "Set status" (Automatic shows the
 * fold's state in brackets) and "Delete league...", disabled while the run
 * reads Active. Every run kind has them all; without `shareable` the two copy
 * items are left out, and with `statusOnly` (an Arena run) only "Set status"
 * is offered.
 */
withDefaults(
  defineProps<{
    canDelete?: boolean
    canAddMatches?: boolean
    /** The player's status; `null` is Automatic. */
    userState?: RunStateView | null
    /** The fold's state. */
    autoState?: RunStateView
    /** Offer the two copy items. */
    shareable?: boolean
    /** Offer "Set status" alone: an Arena run takes no other edit. */
    statusOnly?: boolean
    disabled?: boolean
  }>(),
  {
    canDelete: false,
    canAddMatches: false,
    userState: null,
    autoState: 'active',
    shareable: true,
    statusOnly: false,
    disabled: false,
  },
)
const emit = defineEmits<{
  screenshot: []
  copySummary: []
  editNotes: []
  addMatches: []
  setStatus: [status: RunStateView | null]
  delete: []
}>()
</script>

<template>
  <DropdownMenu>
    <DropdownMenuTrigger as-child>
      <Button
        variant="ghost"
        size="icon"
        class="size-7 shrink-0"
        aria-label="League actions"
        :disabled="disabled"
        @click.stop
      >
        <MoreHorizontal class="size-4" />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end" class="w-64 whitespace-nowrap">
      <template v-if="shareable && !statusOnly">
        <DropdownMenuItem @select="emit('screenshot')">
          <Camera class="mr-2 size-4" />
          Copy screenshot of league
        </DropdownMenuItem>
        <DropdownMenuItem @select="emit('copySummary')">
          <Copy class="mr-2 size-4" />
          Copy league summary
        </DropdownMenuItem>
        <DropdownMenuSeparator />
      </template>
      <DropdownMenuItem v-if="!statusOnly" @select="emit('editNotes')">
        <PencilLine class="mr-2 size-4" />
        Edit notes
      </DropdownMenuItem>
      <DropdownMenuItem v-if="canAddMatches && !statusOnly" @select="emit('addMatches')">
        <ListPlus class="mr-2 size-4" />
        Add matches...
      </DropdownMenuItem>
      <DropdownMenuSub>
        <DropdownMenuSubTrigger>
          <Tag class="mr-2 size-4" />
          Set status
        </DropdownMenuSubTrigger>
        <DropdownMenuSubContent class="w-52">
          <DropdownMenuItem :data-checked="userState === null" @select="emit('setStatus', null)">
            <Check class="mr-2 size-4" :class="userState === null ? '' : 'invisible'" />
            {{ automaticLabel(autoState) }}
          </DropdownMenuItem>
          <DropdownMenuItem
            v-for="choice in STATUS_CHOICES"
            :key="choice.value"
            :data-checked="userState === choice.value"
            @select="emit('setStatus', choice.value)"
          >
            <Check class="mr-2 size-4" :class="userState === choice.value ? '' : 'invisible'" />
            {{ choice.label }}
          </DropdownMenuItem>
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      <template v-if="!statusOnly">
        <DropdownMenuSeparator />
        <DropdownMenuItem class="text-destructive" :disabled="!canDelete" @select="emit('delete')">
          <Trash2 class="mr-2 size-4" />
          Delete league...
          <span v-if="!canDelete" class="ml-auto text-xs text-muted-foreground">Drop it first</span>
        </DropdownMenuItem>
      </template>
    </DropdownMenuContent>
  </DropdownMenu>
</template>
