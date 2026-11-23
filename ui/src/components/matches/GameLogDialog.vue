<script setup lang="ts">
import { useQuery } from '@tanstack/vue-query'
import { ScrollText } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { commands } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog'
import { errorMessage, unwrap } from '@/lib/commands'
import { matchDetailKeys } from '@/lib/matchDetail'

/** The "Game log" button and its dialog (0.x `GameLogPanel` in a dialog). The log loads when opened. */
const props = defineProps<{
  token: string
  gameNumber: number
}>()

const open = ref(false)

const { data: entries, error, isPending } = useQuery({
  queryKey: computed(() => matchDetailKeys.gameLog(props.token, props.gameNumber)),
  queryFn: async ({ queryKey }) => unwrap(await commands.gameLog(queryKey[1], queryKey[3])),
  enabled: open,
})
</script>

<template>
  <Dialog v-model:open="open">
    <DialogTrigger as-child>
      <Button variant="ghost" size="sm" class="h-7 px-2 text-xs">
        <ScrollText :size="12" />
        Game log
      </Button>
    </DialogTrigger>
    <DialogContent class="max-h-[80vh] max-w-lg gap-0 p-0">
      <DialogHeader class="px-4 pt-4 pb-4">
        <DialogTitle>Game {{ gameNumber }} log</DialogTitle>
      </DialogHeader>
      <div class="h-[60vh]">
        <div class="flex h-full flex-col">
          <div class="border-b bg-muted/50 px-3 py-2">
            <span class="text-xs font-medium tracking-wide text-muted-foreground uppercase">Game Log</span>
          </div>
          <div v-if="entries && entries.length" class="flex-1 overflow-y-auto p-2">
            <div
              v-for="(entry, i) in entries"
              :key="i"
              class="border-b border-border/50 px-1 py-1 text-xs text-muted-foreground last:border-0"
            >
              <span class="mr-1.5 font-mono text-[10px] opacity-60">{{ entry.time }}</span>
              <span>{{ entry.message }}</span>
            </div>
          </div>
          <div v-else-if="error" class="flex flex-1 items-center justify-center px-4 text-xs text-destructive">
            Could not load the game log: {{ errorMessage(error) }}
          </div>
          <div v-else-if="isPending" class="flex flex-1 items-center justify-center text-xs text-muted-foreground">
            Loading game log...
          </div>
          <div v-else class="flex flex-1 items-center justify-center text-xs text-muted-foreground">
            No game log available
          </div>
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
