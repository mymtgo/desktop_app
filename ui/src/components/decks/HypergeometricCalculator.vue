<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { TypeGroupView } from '@/bindings'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { drawOddsCards, type DrawOddsCard, resultLines } from '@/lib/decklist'
import { hypergeometric } from '@/lib/hypergeometric'

const props = defineProps<{
  maindeck: TypeGroupView[]
}>()

const cards = computed((): DrawOddsCard[] => drawOddsCards(props.maindeck))

const deckTotal = computed((): number => cards.value.reduce((sum, card) => sum + card.quantity, 0))

const selectedName = ref<string>('')

const selectedCard = computed((): DrawOddsCard | null => cards.value.find((card) => card.name === selectedName.value) ?? null)

// Editable inputs. N starts at the deck total; K refills on a card change.
const populationSize = ref<number | string>(deckTotal.value)
const successesInPopulation = ref<number | string>(0)
const sampleSize = ref<number | string>(7)
const successesInSample = ref<number | string>(1)

// N follows the deck total until a card is chosen; after that it is the user's.
watch(deckTotal, (total) => {
  if (!selectedCard.value) {
    populationSize.value = total
  }
})

// A card change refills K with the card's copies. Edits to K then stand until
// the card changes again.
watch(selectedCard, (card) => {
  if (card) {
    successesInPopulation.value = card.quantity
    if (!populationSize.value) {
      populationSize.value = deckTotal.value
    }
  }
})

const toNumber = (value: number | string): number => {
  const parsed = typeof value === 'number' ? value : parseInt(value, 10)
  return Number.isFinite(parsed) ? parsed : 0
}

const results = computed(() =>
  hypergeometric(
    toNumber(populationSize.value),
    toNumber(successesInPopulation.value),
    toNumber(sampleSize.value),
    toNumber(successesInSample.value),
  ),
)

const lines = computed(() => resultLines(results.value, toNumber(successesInSample.value)))
</script>

<template>
  <div v-if="cards.length" class="flex flex-col gap-2">
    <h3 class="text-sm font-medium text-muted-foreground">
      Draw Odds <span class="text-xs font-normal">(hypergeometric)</span>
    </h3>

    <div class="flex flex-col gap-3 rounded-md border border-border bg-muted/30 px-3 py-3">
      <div class="flex flex-col gap-1.5">
        <Label for="hyp-card" class="text-xs text-muted-foreground">Card</Label>
        <Select :model-value="selectedName || undefined" @update:model-value="(value) => (selectedName = String(value ?? ''))">
          <SelectTrigger id="hyp-card" class="h-8 w-full text-sm">
            <SelectValue placeholder="Select a card…" />
          </SelectTrigger>
          <SelectContent>
          <SelectItem v-for="card in cards" :key="card.name" :value="card.name">
            {{ card.name }}
          </SelectItem>
          </SelectContent>
        </Select>
      </div>

      <div class="grid grid-cols-2 gap-2">
        <div class="flex flex-col gap-1.5">
          <Label for="hyp-n" class="text-xs text-muted-foreground">Population Size</Label>
          <Input
            id="hyp-n"
            v-model.number="populationSize"
            type="number"
            min="0"
            :disabled="!selectedCard"
            class="h-8 tabular-nums"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="hyp-k-pop" class="text-xs text-muted-foreground">Successes in Pop.</Label>
          <Input
            id="hyp-k-pop"
            v-model.number="successesInPopulation"
            type="number"
            min="0"
            :disabled="!selectedCard"
            class="h-8 tabular-nums"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="hyp-sample" class="text-xs text-muted-foreground">Sample Size</Label>
          <Input
            id="hyp-sample"
            v-model.number="sampleSize"
            type="number"
            min="0"
            :disabled="!selectedCard"
            class="h-8 tabular-nums"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="hyp-k" class="text-xs text-muted-foreground">Successes in Sample</Label>
          <Input
            id="hyp-k"
            v-model.number="successesInSample"
            type="number"
            min="0"
            :disabled="!selectedCard"
            class="h-8 tabular-nums"
          />
        </div>
      </div>

      <div
        class="flex flex-col gap-1.5 border-t border-border pt-3 transition-opacity"
        :class="selectedCard ? 'opacity-100' : 'pointer-events-none opacity-40'"
      >
        <div v-for="line in lines" :key="line.label" class="flex items-baseline justify-between gap-2">
          <span class="text-xs text-muted-foreground">{{ line.label }}</span>
          <span class="text-sm font-semibold tabular-nums">{{ selectedCard ? line.value : '-' }}</span>
        </div>
      </div>
    </div>
  </div>
</template>
