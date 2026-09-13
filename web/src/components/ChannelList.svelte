<script lang="ts">
  import { chat } from "../lib/chat.svelte";
  import StatusDot from "./StatusDot.svelte";

  let {
    onInvite,
    onManageChannels,
  }: { onInvite: () => void; onManageChannels: () => void } = $props();
</script>

<aside
  class="flex w-56 shrink-0 flex-col border-r border-[var(--color-edge)] bg-[var(--color-panel)]"
>
  <header class="border-b border-[var(--color-edge)] px-4 py-3">
    <h2 class="truncate text-sm font-semibold">
      {chat.activeServerSummary?.name ?? "No space"}
    </h2>
    <div class="mt-1 flex items-center gap-2">
      <StatusDot status={chat.activeStatus} label />
    </div>
  </header>

  {#if chat.amOwner}
    <button
      onclick={onManageChannels}
      class="mx-2 mt-2 rounded border border-dashed border-[var(--color-edge)] px-2 py-1 text-xs text-neutral-500 transition hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
      title="Add or remove channels"
    >
      + channel
    </button>
  {/if}

  <ul class="flex-1 overflow-y-auto p-2">
    {#each chat.channels as channel (channel.id)}
      {@const unread = chat.unread[channel.id] ?? 0}
      <li>
        <button
          class="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-sm transition
            {chat.activeChannel === channel.id
            ? 'bg-[var(--color-edge)] text-neutral-100'
            : unread > 0
              ? 'font-semibold text-neutral-100 hover:bg-white/5'
              : 'text-neutral-400 hover:bg-white/5 hover:text-neutral-200'}"
          onclick={() => chat.selectChannel(channel.id)}
        >
          <span class="min-w-0 flex-1 truncate">
            <span class="text-neutral-600">#</span>
            {channel.name}
          </span>
          <!-- Only on channels you are not looking at. A badge on the open
               channel would count messages you are reading as unread. -->
          {#if unread > 0 && chat.activeChannel !== channel.id}
            <span
              class="shrink-0 rounded-full bg-[var(--color-accent)] px-1.5 py-0.5 text-[10px] font-bold leading-none text-black"
              aria-label="{unread} unread"
            >
              {unread > 99 ? "99+" : unread}
            </span>
          {/if}
        </button>
      </li>
    {:else}
      <li class="px-2 py-1.5 text-xs text-neutral-600">no channels yet</li>
    {/each}
  </ul>

  <footer
    class="flex items-center justify-between border-t border-[var(--color-edge)] px-3 py-2 text-xs text-neutral-500"
  >
    <span class="truncate" title={chat.activeServerSummary?.username}>
      {chat.activeServerSummary?.username ?? ""}
    </span>
    <button
      class="rounded px-2 py-1 text-neutral-400 transition hover:bg-white/5 hover:text-[var(--color-accent)] disabled:opacity-40"
      disabled={chat.activeStatus === "offline"}
      onclick={onInvite}
      title="Mint an invite code"
    >
      invite
    </button>
  </footer>
</aside>
