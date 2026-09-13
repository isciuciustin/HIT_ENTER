<script lang="ts">
  // Creating a channel, and deleting one.
  //
  // The two live in one dialog because they are the same decision seen from
  // both ends, and because deleting needs somewhere to say what it costs —
  // a delete takes every message in the channel with it, which is not
  // something to learn from a hover tooltip.

  import { chat } from "../lib/chat.svelte";
  import Dialog from "./Dialog.svelte";

  let { onClose }: { onClose: () => void } = $props();

  let name = $state("");
  let topic = $state("");
  let busy = $state(false);
  /** The channel the user has asked to delete, awaiting a second click. */
  let confirming = $state<string | null>(null);

  const nameMax = $derived(chat.limits.channel_name_max_chars);
  const tooLong = $derived(name.length > nameMax);
  const canCreate = $derived(name.trim().length > 0 && !tooLong && !busy);
  /** A space must keep one channel; the server refuses the last either way. */
  const canDelete = $derived(chat.channels.length > 1);

  async function create() {
    if (!canCreate) return;
    busy = true;
    const ok = await chat.createChannel(name, topic);
    busy = false;
    if (ok) onClose();
  }

  async function remove(channelId: string) {
    if (confirming !== channelId) {
      confirming = channelId;
      return;
    }
    busy = true;
    await chat.deleteChannel(channelId);
    busy = false;
    confirming = null;
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void create();
    }
  }
</script>

<Dialog {onClose}>
  <h2 class="text-lg font-semibold">Channels</h2>
  <p class="mt-1 text-sm text-neutral-400">
    Only you can add and remove these — you host this space.
  </p>

  <div class="mt-4 space-y-2">
    <label class="block text-xs uppercase tracking-wide text-neutral-500" for="channel-name">
      new channel
    </label>
    <input
      id="channel-name"
      bind:value={name}
      onkeydown={onKeydown}
      placeholder="scratch"
      maxlength={nameMax + 1}
      class="w-full rounded border bg-black/30 px-3 py-2 text-sm text-neutral-100 placeholder:text-neutral-700 focus:outline-none
        {tooLong ? 'border-red-500/60' : 'border-[var(--color-edge)]'}"
    />
    <input
      bind:value={topic}
      onkeydown={onKeydown}
      placeholder="topic — optional, one line"
      class="w-full rounded border border-[var(--color-edge)] bg-black/30 px-3 py-2 text-sm text-neutral-100 placeholder:text-neutral-700 focus:outline-none"
    />
  </div>

  <ul class="mt-5 space-y-1 border-t border-[var(--color-edge)] pt-4">
    {#each chat.channels as channel (channel.id)}
      <li class="flex items-center gap-2 rounded px-2 py-1.5 text-sm">
        <span class="min-w-0 flex-1 truncate">
          <span class="text-neutral-600">#</span>{channel.name}
          {#if channel.topic}
            <span class="ml-2 text-xs text-neutral-600">{channel.topic}</span>
          {/if}
        </span>
        {#if confirming === channel.id}
          <span class="shrink-0 text-[11px] text-red-400">
            deletes every message in it
          </span>
        {/if}
        <button
          onclick={() => remove(channel.id)}
          disabled={!canDelete || busy}
          class="shrink-0 rounded px-2 py-0.5 text-[11px] transition disabled:opacity-30
            {confirming === channel.id
            ? 'bg-red-500/20 text-red-300'
            : 'text-neutral-500 hover:text-red-400'}"
          title={canDelete
            ? "Delete this channel and its messages"
            : "A space has to keep at least one channel"}
        >
          {confirming === channel.id ? "really delete" : "delete"}
        </button>
      </li>
    {/each}
  </ul>

  {#if chat.error}
    <p class="mt-4 rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-sm text-red-300">
      {chat.error.message}
    </p>
  {/if}

  <div class="mt-5 flex justify-end gap-2">
    <button
      onclick={onClose}
      class="rounded px-3 py-1.5 text-sm text-neutral-400 transition hover:text-neutral-200"
    >
      close
    </button>
    <button
      onclick={create}
      disabled={!canCreate}
      class="rounded bg-[var(--color-accent)] px-3 py-1.5 text-sm font-semibold text-black transition disabled:opacity-40"
    >
      {busy ? "…" : "create"}
    </button>
  </div>
</Dialog>
