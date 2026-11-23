<script setup lang="ts">
import AccountsCard from '@/components/settings/AccountsCard.vue'
import BackgroundCard from '@/components/settings/BackgroundCard.vue'
import InterfaceScaleCard from '@/components/settings/InterfaceScaleCard.vue'
import UpdatesCard from '@/components/UpdatesCard.vue'
import { useWhatsNewUrl } from '@/composables/useWhatsNew'

// 0.x `pages/settings/General.vue`: Updates, Accounts (shown with at least
// one account), then Background.
const whatsNewUrl = useWhatsNewUrl()
</script>

<template>
  <div class="flex flex-col gap-3">
    <UpdatesCard :whats-new-url="whatsNewUrl" />
    <AccountsCard />
    <InterfaceScaleCard />
    <BackgroundCard />
  </div>
</template>
