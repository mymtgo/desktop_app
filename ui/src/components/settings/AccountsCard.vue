<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { computed, ref } from 'vue'
import { commands } from '@/bindings'
import { Badge } from '@/components/ui/badge'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { keys } from '@/lib/keys'
import { useAccounts } from '@/lib/queries'

// 0.x `components/settings/AccountsCard.vue` (release spec 5), shown with at
// least one account. An untracked account's new matches are not built; they
// come back when tracking is turned on again.
const queryClient = useQueryClient()
const { add } = useToast()
const { data: view } = useAccounts()
const processing = ref<string | null>(null)

const accounts = computed(() => view.value?.accounts ?? [])

async function toggleTracking(username: string, tracked: boolean) {
  processing.value = username
  try {
    queryClient.setQueryData(keys.accounts, unwrap(await commands.setAccountTracked(username, tracked)))
  } catch (error) {
    add({ type: 'error', title: 'Could not change tracking', message: errorMessage(error) })
  } finally {
    processing.value = null
  }
}
</script>

<template>
  <section v-if="accounts.length" class="flex flex-col gap-4 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">Accounts</h2>
      <p class="text-xs text-muted-foreground">Toggle tracking to control which accounts record match data.</p>
    </div>
    <div
      v-for="account in accounts"
      :key="account.id"
      :data-account="account.username"
      class="flex items-center justify-between gap-4"
    >
      <div>
        <Label>
          {{ account.label }}
          <Badge v-if="account.active" variant="default" class="ml-1 text-xs">Active</Badge>
        </Label>
        <p class="text-sm text-muted-foreground">
          {{ account.tracked ? 'Recording matches' : 'Not recording matches' }}
        </p>
      </div>
      <Switch
        :model-value="account.tracked"
        :disabled="processing === account.username"
        @update:model-value="(value: boolean) => toggleTracking(account.username, value)"
      />
    </div>
  </section>
</template>
