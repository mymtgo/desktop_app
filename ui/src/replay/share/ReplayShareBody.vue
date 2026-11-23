<script setup lang="ts">
import { Check, Copy, Lock } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { DialogFooter } from '@/components/ui/dialog'
import type { ReplayShareView } from './types'

/** The share dialog's content (0.x's `ReplayShareDialog`), apart from the dialog frame. */
defineProps<ReplayShareView>()
const emit = defineEmits<{ share: []; revoke: []; copy: [] }>()
</script>

<template>
  <div class="grid gap-4">
    <p v-if="share && !share.linked" class="text-sm text-muted-foreground">
      Sign in on the Account page to share replays.
    </p>
    <p
      v-else-if="share && !share.supporter && !share.url"
      class="flex items-center gap-2 text-sm text-muted-foreground"
    >
      <Lock :size="14" />
      Sharing replays is a supporter feature.
    </p>

    <template v-if="share?.url">
      <div class="flex items-center gap-2">
        <input
          readonly
          :value="share.url"
          aria-label="Replay link"
          class="h-9 min-w-0 flex-1 rounded-md border border-black/60 bg-input/30 px-3 font-mono text-xs"
          @focus="($event.target as HTMLInputElement).select()"
        />
        <Button type="button" variant="secondary" size="sm" @click="emit('copy')">
          <Check v-if="copied" />
          <Copy v-else />
          {{ copied ? 'Copied' : 'Copy link' }}
        </Button>
      </div>
    </template>

    <p v-if="error" class="text-sm text-destructive">{{ error }}</p>

    <p v-if="share?.url" class="text-xs text-muted-foreground">
      Played more games since sharing? Update the link to add them.
    </p>

    <DialogFooter>
      <Button v-if="share?.url" type="button" variant="ghost" size="sm" :disabled="busy" @click="emit('revoke')">
        Switch off link
      </Button>
      <Button
        v-if="share?.linked"
        type="button"
        :variant="share.url ? 'secondary' : 'default'"
        size="sm"
        :disabled="busy"
        @click="emit('share')"
      >
        {{ busy ? 'Uploading...' : share.url ? 'Update link' : 'Create link' }}
      </Button>
    </DialogFooter>
  </div>
</template>
