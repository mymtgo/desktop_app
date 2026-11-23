<script setup lang="ts">
import { computed, shallowRef, watch } from 'vue';
import { cardImageSrc } from '@/lib/cards';

const props = defineProps<{
    name?: string | null;
    type?: string | null;
    image?: string | null;
    /** The downloaded image file (v1, Plan S): shown through the `card` scheme before `image`. */
    local?: string | null;
    /** Show the type line on the fallback face. */
    showType?: boolean;
}>();

/** The local file failed to load (a moved or missing folder), so the remote image shows instead. */
const localFailed = shallowRef(false);
const failed = shallowRef(false);

const src = computed(() => (localFailed.value ? (props.image ?? null) : cardImageSrc(props.image ?? null, props.local ?? null)));

function onError() {
    if (props.local && !localFailed.value) {
        localFailed.value = true;

        return;
    }

    failed.value = true;
}

watch(
    () => [props.image, props.local],
    () => {
        localFailed.value = false;
        failed.value = false;
    },
);
</script>

<template>
    <div class="absolute inset-0 overflow-hidden rounded-[inherit] bg-linear-160 from-[#3b3a35] to-[#23221f]">
        <div class="absolute inset-0 flex flex-col justify-between p-[8%] text-[10px] leading-tight text-[#eee]">
            <span class="font-semibold">{{ name ?? 'Unknown card' }}</span>
            <span v-if="showType && type" class="text-[9px] text-[#cfcfcf]">{{ type }}</span>
        </div>
        <img
            v-if="src && !failed"
            :src="src"
            :alt="name ?? 'card'"
            draggable="false"
            class="absolute inset-0 block size-full object-cover"
            @error="onError"
        />
    </div>
</template>
