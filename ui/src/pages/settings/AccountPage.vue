<script setup lang="ts">
import { ref } from 'vue'
import DeckSyncList from '@/components/settings/DeckSyncList.vue'
import SyncCard from '@/components/settings/SyncCard.vue'

// 0.x settings/Account: the account card reports whether this device is
// signed in, and the deck list below follows it.
const linked = ref(false)
</script>

<template>
  <div class="flex flex-col gap-3">

    <SyncCard @update:linked="linked = $event" />
    <DeckSyncList :linked="linked" />
  </div>
</template>
