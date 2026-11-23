<script setup lang="ts">
// 0.x `ArchetypeForm.vue`: name, format, colour identity and the source of
// the cards (a .dek file or a match's opponent cards).
import { computed, ref, watch } from 'vue'
import { commands, type ArchetypeFormInput, type PrefillView, type ResolvedCardView } from '@/bindings'
import DekUploadButton from '@/components/archetypes/DekUploadButton.vue'
import ManaIdentityPicker from '@/components/archetypes/ManaIdentityPicker.vue'
import MatchSelect from '@/components/archetypes/MatchSelect.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { ARCHETYPE_FORMATS, NAME_MAX, formCards, formProblem, submitButtonLabel } from '@/lib/archetypes'
import { errorMessage, unwrap } from '@/lib/commands'

const props = withDefaults(
  defineProps<{
    name?: string
    format?: string | null
    colorIdentity?: string | null
    submitLabel: string
    prefill?: PrefillView | null
    requireCards?: boolean
    /** A commander archetype: only the name is edited; the format and colours go back as stored. */
    nameOnly?: boolean
    cancelTo: string
    submitting?: boolean
    /** The server's refusal of the last submit. */
    serverError?: string | null
  }>(),
  { name: '', format: null, colorIdentity: null, prefill: null, requireCards: false, nameOnly: false, submitting: false, serverError: null },
)

const emit = defineEmits<{ submit: [form: ArchetypeFormInput] }>()

const name = ref(props.name)
const format = ref(props.prefill?.format ?? props.format ?? '')
const colorIdentity = ref<string | null>(props.prefill?.cards.colorIdentity ?? props.colorIdentity)
const cards = ref<ResolvedCardView[]>(props.prefill?.cards.cards ?? [])
const sourceMatchId = ref<number | null>(props.prefill?.sourceMatchId ?? null)
const incomplete = ref(props.prefill !== null)
const scanning = ref(false)
const scanError = ref<string | null>(null)
// The prefill's match is already scanned.
let skipNextScan = props.prefill !== null

const label = computed(() => submitButtonLabel(props.requireCards, cards.value.length, props.submitLabel))
const problem = computed(() =>
  formProblem({ name: name.value, format: format.value, cardCount: cards.value.length, requireCards: props.requireCards }),
)

function applyResolved(data: { cards: ResolvedCardView[]; colorIdentity: string | null }) {
  cards.value = data.cards
  if (data.colorIdentity) colorIdentity.value = data.colorIdentity
}

function onDekResolved(data: { cards: ResolvedCardView[]; colorIdentity: string | null }) {
  sourceMatchId.value = null
  incomplete.value = false
  scanError.value = null
  applyResolved(data)
}

watch(sourceMatchId, async (matchId) => {
  if (matchId === null) return
  if (skipNextScan) {
    skipNextScan = false
    return
  }
  scanning.value = true
  scanError.value = null
  try {
    applyResolved(unwrap(await commands.scanMatchCards(matchId)))
    incomplete.value = true
  } catch (failure) {
    scanError.value = errorMessage(failure) || 'Failed to scan match.'
    sourceMatchId.value = null
  } finally {
    scanning.value = false
  }
})

function submit() {
  if (problem.value !== null) return
  emit('submit', {
    name: name.value.trim(),
    format: format.value,
    colorIdentity: colorIdentity.value,
    cards: formCards(cards.value),
    sourceMatchId: sourceMatchId.value,
    incomplete: incomplete.value,
  })
}
</script>

<template>
  <form class="flex w-72 shrink-0 flex-col gap-5" @submit.prevent="submit">
    <div>
      <Label for="archetype-name">Archetype Name</Label>
      <Input id="archetype-name" v-model="name" placeholder="e.g. Mono Red Aggro" class="mt-1.5" :maxlength="NAME_MAX * 2" required />
    </div>

    <template v-if="!nameOnly">
      <div>
        <Label for="archetype-format">Format</Label>
        <Select :model-value="format || undefined" @update:model-value="(value) => (format = String(value ?? ''))">
          <SelectTrigger id="archetype-format" class="mt-1.5 w-full">
            <SelectValue placeholder="Select format..." />
          </SelectTrigger>
          <SelectContent>
            <SelectItem v-for="option in ARCHETYPE_FORMATS" :key="option.value" :value="option.value">{{ option.label }}</SelectItem>
          </SelectContent>
        </Select>
      </div>

      <div>
        <Label>Color Identity</Label>
        <ManaIdentityPicker v-model="colorIdentity" class="mt-2" />
        <p class="mt-1.5 text-xs text-muted-foreground">Auto-populated from deck. Click to toggle.</p>
      </div>

      <div>
        <Label>Source</Label>
        <div class="mt-1.5 space-y-3">
          <DekUploadButton @resolved="onDekResolved" />
          <div class="relative flex items-center gap-2">
            <div class="h-px flex-1 bg-border"></div>
            <span class="text-xs text-muted-foreground">or</span>
            <div class="h-px flex-1 bg-border"></div>
          </div>
          <MatchSelect v-model="sourceMatchId" :format="format" />
          <p v-if="scanning" class="text-xs text-muted-foreground">Scanning match...</p>
          <p v-if="scanError" class="text-xs text-red-400">{{ scanError }}</p>
          <p v-if="cards.length" class="text-xs text-muted-foreground">
            {{ cards.length }} card {{ cards.length === 1 ? 'row' : 'rows' }} loaded.
          </p>
        </div>
      </div>

      <ul v-if="cards.length" class="max-h-56 overflow-y-auto rounded-md border text-xs">
        <li v-for="(card, index) in cards" :key="`${card.catalogId}-${card.sideboard}-${index}`" class="flex justify-between gap-2 border-b px-2 py-1 last:border-b-0">
          <span class="truncate">{{ card.quantity }} {{ card.name }}</span>
          <span v-if="card.sideboard" class="text-muted-foreground">Sideboard</span>
        </li>
      </ul>
    </template>

    <p v-if="serverError" class="text-xs text-red-400">{{ serverError }}</p>

    <div class="flex gap-3 pt-4">
      <Button type="submit" :disabled="submitting || scanning || problem !== null" :title="problem ?? undefined">
        {{ label }}
      </Button>
      <Button variant="outline" as-child>
        <RouterLink :to="cancelTo">Cancel</RouterLink>
      </Button>
    </div>
  </form>
</template>
