<script setup lang="ts">
// A boxed pane: a one-pixel frame with the title set into its top border,
// highlighted while focus is inside it (the focused pane of a TUI).
defineProps<{ title: string; focused: boolean }>()
</script>

<template>
  <section class="tui-pane" :class="{ 'tui-pane-focused': focused }">
    <header class="tui-pane-title">
      <span class="tui-pane-name">{{ title }}</span>
      <slot name="title" />
    </header>
    <div class="tui-pane-body">
      <slot />
    </div>
  </section>
</template>
