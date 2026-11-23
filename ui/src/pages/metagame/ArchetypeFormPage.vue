<script setup lang="ts">
// Create (`/metagame/archetypes/new`, optional `?source_match_id=` and
// `?format=`) and edit (`/metagame/archetypes/:id/edit`) a manual
// archetype (0.x `archetypes/Create.vue`, `Edit.vue`).
import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { commands, type ArchetypeFormInput } from '@/bindings'
import ArchetypeForm from '@/components/archetypes/ArchetypeForm.vue'
import VariantDecklist from '@/components/archetypes/VariantDecklist.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { MANAGE_PATH, archetypeDetailPath, archetypeKeys, useArchetypeForm } from '@/lib/archetypes'
import { errorMessage, unwrap } from '@/lib/commands'

const route = useRoute()
const router = useRouter()
const queryClient = useQueryClient()

const editId = computed<number | null>(() => {
  const raw = route.params.id
  const id = Number(Array.isArray(raw) ? raw[0] : raw)
  return Number.isInteger(id) && id > 0 ? id : null
})
const creating = computed(() => editId.value === null)

function queryNumber(value: unknown): number | null {
  const id = Number(Array.isArray(value) ? value[0] : value)
  return Number.isInteger(id) && id > 0 ? id : null
}
const sourceMatchId = computed(() => queryNumber(route.query.source_match_id))
const queryFormat = computed(() => (typeof route.query.format === 'string' ? route.query.format : null))

const prefill = useQuery({
  queryKey: computed(() => archetypeKeys.prefill(sourceMatchId.value ?? 0)),
  queryFn: async ({ queryKey }) => unwrap(await commands.archetypeCreatePrefill(queryKey[2])),
  enabled: computed(() => creating.value && sourceMatchId.value !== null),
  staleTime: Infinity,
})

const form = useArchetypeForm(editId)

const submitting = ref(false)
const serverError = ref<string | null>(null)
// A new key remounts the form after a saved edit, so its cards clear.
const formKey = ref(0)

async function submit(input: ArchetypeFormInput) {
  submitting.value = true
  serverError.value = null
  try {
    if (editId.value === null) {
      const created = unwrap(await commands.createArchetype(input))
      // Every page that names archetypes may show this one (T7 review m4).
      await queryClient.invalidateQueries()
      // The form leaves history, so Back returns to where the player came from.
      await router.replace(archetypeDetailPath(created.id))
    } else {
      unwrap(await commands.updateArchetype(editId.value, { ...input, sourceMatchId: null, incomplete: false }))
      await queryClient.invalidateQueries()
      formKey.value += 1
    }
  } catch (failure) {
    serverError.value = errorMessage(failure) || 'An error occurred.'
  } finally {
    submitting.value = false
  }
}

async function refresh() {
  await queryClient.invalidateQueries()
}

const ready = computed(() => (creating.value ? sourceMatchId.value === null || !prefill.isPending.value : !form.isPending.value))
</script>

<template>
  <div class="flex min-h-0 flex-col p-3 lg:p-4">
    <div class="mb-6 border-b pb-4">
      <h1 class="text-lg font-bold">
        {{ creating ? 'Create Archetype' : `Edit ${form.data.value?.name ?? ''}` }}
      </h1>
    </div>

    <template v-if="!ready">
      <Skeleton class="h-64 w-72" />
    </template>

    <template v-else-if="creating">
      <!-- T7 review m7: a failed prefill says why instead of an empty form. -->
      <p v-if="prefill.error.value" class="mb-4 text-sm text-red-400">
        Could not load the match's cards: {{ errorMessage(prefill.error.value) }}
      </p>
      <ArchetypeForm
        :prefill="prefill.data.value ?? null"
        :format="queryFormat"
        submit-label="Create Archetype"
        require-cards
        :cancel-to="MANAGE_PATH"
        :submitting="submitting"
        :server-error="serverError"
        @submit="submit"
      />
    </template>

    <p v-else-if="!form.data.value" class="text-sm text-muted-foreground">This archetype does not exist.</p>

    <div v-else class="flex min-h-0 flex-1 gap-8">
      <ArchetypeForm
        :key="`${form.data.value.id}-${formKey}`"
        :name="form.data.value.name"
        :format="form.data.value.format"
        :color-identity="form.data.value.colorIdentity"
        submit-label="Save Changes"
        :name-only="form.data.value.source === 'commander'"
        :cancel-to="archetypeDetailPath(form.data.value.id)"
        :submitting="submitting"
        :server-error="serverError"
        @submit="submit"
      />
      <p v-if="form.data.value.source === 'commander'" class="text-sm text-muted-foreground">
        This archetype is made from its commanders. You can rename it or merge it, but not change its colours,
        format or lists.
      </p>
      <div v-else class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto">
        <div v-if="form.data.value.variants.length === 0" class="flex h-full items-center justify-center text-sm text-muted-foreground">
          No variants yet. Use the form to add one.
        </div>
        <VariantDecklist
          v-for="(variant, index) in form.data.value.variants"
          :key="variant.list"
          :archetype-id="form.data.value.id"
          :archetype-name="form.data.value.name"
          :list="variant.list"
          :variant-label="`Variant ${index + 1}`"
          :cards="variant.cards"
          :editable="form.data.value.source === 'manual'"
          :can-remove="form.data.value.variants.length > 1"
          @changed="refresh"
        />
      </div>
    </div>
  </div>
</template>
