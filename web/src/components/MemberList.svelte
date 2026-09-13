<script lang="ts">
  import { chat } from "../lib/chat.svelte";

  // The roster, with a dot for who is here right now.
  //
  // The names come from the mirror, so this panel is correct with the network
  // off — an empty member list is exactly when you are trying to work out who
  // said something. Presence does *not* come from the mirror and never will:
  // "online" is a property of an open connection, and a stored one would be a
  // lie the moment the app closed (PLAN §6).

  let {
    onClose,
    onOpenMember,
  }: { onClose: () => void; onOpenMember: (id: string) => void } = $props();

  const offlineSpace = $derived(chat.activeStatus === "offline");
</script>

<aside
  class="flex w-52 shrink-0 flex-col border-l border-[var(--color-edge)] bg-[var(--color-panel)]"
>
  <header
    class="flex items-center justify-between border-b border-[var(--color-edge)] px-3 py-3"
  >
    <h2 class="text-xs font-semibold uppercase tracking-wide text-neutral-400">
      members
      <span class="ml-1 font-normal text-neutral-600">
        {#if offlineSpace}
          {chat.members.length}
        {:else}
          {chat.onlineCount}/{chat.members.length}
        {/if}
      </span>
    </h2>
    <button
      onclick={onClose}
      class="rounded px-1.5 py-0.5 text-xs text-neutral-500 transition hover:text-neutral-200"
      title="Hide members"
      aria-label="Hide members"
    >
      ×
    </button>
  </header>

  <ul class="flex-1 overflow-y-auto p-2">
    {#each chat.roster as member (member.id)}
      <li>
        <!-- Clickable for everyone, not only the owner: your own row is where
             you see and revoke your own machines. -->
        <button
          onclick={() => onOpenMember(member.id)}
          class="flex w-full items-center gap-2 rounded px-2 py-1 text-left text-sm transition hover:bg-white/5
            {member.banned
            ? 'text-neutral-700 line-through'
            : member.online || offlineSpace
              ? 'text-neutral-300'
              : 'text-neutral-600'}"
          title={member.banned ? `${member.username} — banned` : member.username}
        >
          <span
            class="size-1.5 shrink-0 rounded-full
              {offlineSpace || member.banned
              ? 'bg-neutral-700'
              : member.online
                ? 'bg-[var(--color-accent)]'
                : 'bg-neutral-700'}"
            aria-hidden="true"
          ></span>
          <span class="min-w-0 flex-1 truncate">
            {member.display_name ?? member.username}
          </span>
          {#if member.banned}
            <span class="shrink-0 text-[10px] text-red-400/70">banned</span>
          {:else if member.id === chat.myId}
            <span class="shrink-0 text-[10px] text-neutral-600">you</span>
          {:else if member.is_owner}
            <span class="shrink-0 text-[10px] text-neutral-600">host</span>
          {/if}
        </button>
      </li>
    {:else}
      <li class="px-2 py-1.5 text-xs text-neutral-600">nobody here yet</li>
    {/each}
  </ul>

  {#if offlineSpace && chat.members.length > 0}
    <!-- Greying everyone out would be a claim we cannot support: with no
         connection we do not know who is here, which is different from
         knowing that nobody is. -->
    <p class="border-t border-[var(--color-edge)] px-3 py-2 text-[11px] text-neutral-600">
      offline — who is here is unknown
    </p>
  {/if}
</aside>
