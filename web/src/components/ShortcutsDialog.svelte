<script lang="ts">
  import { SHORTCUTS } from "../lib/shortcuts";
  import Dialog from "./Dialog.svelte";

  let { onClose }: { onClose: () => void } = $props();
</script>

<Dialog {onClose}>
  <h2 class="text-base font-semibold">Keyboard</h2>

  {#each SHORTCUTS as group (group.group)}
    <h3 class="mt-4 text-[11px] uppercase tracking-wide text-neutral-500">
      {group.group}
    </h3>
    <dl class="mt-1.5 space-y-1">
      {#each group.items as item (item.keys)}
        <div class="flex items-baseline gap-3 text-sm">
          <dt class="w-44 shrink-0 font-mono text-xs text-neutral-300">
            {item.keys}
          </dt>
          <dd class="text-neutral-500">{item.description}</dd>
        </div>
      {/each}
    </dl>
  {/each}

  <div class="mt-5 flex justify-end">
    <button
      onclick={onClose}
      class="rounded px-3 py-1.5 text-xs text-neutral-400 transition hover:bg-white/5"
    >
      close
    </button>
  </div>
</Dialog>
