<script setup lang="ts">
import { Crown } from 'lucide-vue-next'
import { computed } from 'vue'
import type { CommanderCardView } from '@/bindings'
import CardImage from '@/components/CardImage.vue'
import ManaColors from '@/components/mana/ManaColors.vue'

/**
 * A Brawl match's opponent commander above the reveals (Arena Brawl spec):
 * the full card, so its rules text can be read; partners side by side. A
 * commander without a card image shows its name and colours instead.
 */
const props = defineProps<{ commanders: CommanderCardView[] }>()

const title = computed(() => (props.commanders.length > 1 ? "Opponent's commanders" : "Opponent's commander"))

function hasImage(commander: CommanderCardView): boolean {
  return Boolean(commander.image || commander.localImage)
}
</script>

<template>
  <section
    class="flex flex-col overflow-hidden rounded-md border border-white/5 bg-black/40 text-card-foreground shadow outline-1 outline-black/60"
  >
    <header class="panel-header gap-2">
      <span class="inline-flex items-center gap-1.5 text-[11px] font-semibold tracking-widest uppercase">
        <Crown :size="13" class="text-muted-foreground" />
        {{ title }}
      </span>
    </header>
    <div class="grid gap-2 p-3" :class="commanders.length > 1 ? 'grid-cols-2' : 'grid-cols-1'">
      <div v-for="commander in commanders" :key="commander.catalogId" :data-commander="commander.catalogId">
        <CardImage
          v-if="hasImage(commander)"
          :src="commander.image"
          :local="commander.localImage"
          :alt="commander.name"
          loading="lazy"
          decoding="async"
          class="aspect-[488/680] w-full rounded-[4.5%/3.3%] object-cover shadow-md"
        />
        <!-- No card image: a plain card face with the name and colours. -->
        <div
          v-else
          class="flex aspect-[488/680] w-full flex-col gap-1.5 rounded-[4.5%/3.3%] border border-border bg-muted px-2.5 py-2 text-xs"
        >
          <span class="font-semibold break-words">{{ commander.name }}</span>
          <ManaColors v-if="commander.colors.length" :colors="commander.colors" size="xs" />
        </div>
      </div>
    </div>
  </section>
</template>
