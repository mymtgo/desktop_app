<script setup lang="ts">
import { Layers, PencilLine } from 'lucide-vue-next'
import { computed } from 'vue'
import type { SideboardChangeView } from '@/bindings'
import CardTile from '@/components/matches/CardTile.vue'
import CardTilePill from '@/components/matches/CardTilePill.vue'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { changeLabel, sideboardCount } from '@/lib/matchDetail'

/**
 * Cards boarded in and out for game 2 and later (0.x `MatchGame.vue`). A
 * manual match's changes have an Add or Edit button.
 */
const props = withDefaults(
  defineProps<{
    changes: SideboardChangeView[]
    manual?: boolean
  }>(),
  { manual: false },
)

const emit = defineEmits<{ edit: [] }>()

const count = computed(() => sideboardCount(props.changes))
</script>

<template>
  <Card class="gap-0 py-0">
    <header class="panel-header">
      <Layers :size="13" class="text-muted-foreground" />
      <h3 class="text-[11px] font-semibold tracking-widest uppercase">Sideboard changes</h3>
      <span v-if="count" class="font-mono text-[11px] text-muted-foreground">({{ count }})</span>
      <Button v-if="manual" variant="ghost" size="sm" class="h-6 px-1.5 text-[11px]" @click="emit('edit')">
        <PencilLine :size="11" />
        {{ changes.length ? 'Edit' : 'Add' }}
      </Button>
    </header>

    <div v-if="changes.length" class="grid grid-cols-7 gap-1.5 p-4">
      <div
        v-for="change in changes"
        :key="`${change.kind}_${change.catalogId}_${change.name}`"
        class="flex flex-col items-center gap-1"
      >
        <div
          class="aspect-[63/88] w-full overflow-hidden rounded-md shadow-sm"
          :class="change.kind === 'out' ? 'opacity-60' : ''"
        >
          <CardTile :name="change.name" :image="change.image" :local="change.localImage" />
        </div>
        <CardTilePill :tone="change.kind">{{ changeLabel(change) }}</CardTilePill>
      </div>
    </div>
    <p v-else class="m-4 rounded-md border border-dashed px-4 py-5 text-center text-xs text-muted-foreground italic">
      {{ manual ? 'No sideboard changes recorded for this game.' : 'No sideboard changes this game.' }}
    </p>
  </Card>
</template>
