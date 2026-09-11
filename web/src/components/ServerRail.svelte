<script lang="ts">
  import { chat } from "../lib/chat.svelte";
  import StatusDot from "./StatusDot.svelte";

  let { onAdd }: { onAdd: () => void } = $props();

  // Servers are keyed by public key, which is unreadable by design, so the
  // rail shows initials and puts the name in a tooltip.
  function initials(name: string) {
    return name
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? "")
      .join("");
  }
</script>

<nav
  class="flex w-16 shrink-0 flex-col items-center gap-2 border-r border-[var(--color-edge)] bg-black/20 py-3"
  aria-label="Spaces"
>
  {#each chat.servers as server (server.endpoint_id)}
    <button
      class="relative grid h-11 w-11 place-items-center rounded-2xl border text-sm font-semibold transition
        {chat.activeServer === server.endpoint_id
        ? 'border-[var(--color-accent)] bg-[var(--color-panel)] text-[var(--color-accent)]'
        : 'border-transparent bg-[var(--color-panel)] text-neutral-400 hover:border-[var(--color-edge)] hover:text-neutral-200'}"
      title="{server.name} — {server.username}"
      onclick={() => chat.selectServer(server.endpoint_id)}
    >
      {initials(server.name) || "?"}
      <span class="absolute -bottom-0.5 -right-0.5">
        <StatusDot status={chat.status[server.endpoint_id] ?? "offline"} />
      </span>
    </button>
  {/each}

  <button
    class="grid h-11 w-11 place-items-center rounded-2xl border border-dashed border-[var(--color-edge)] text-xl text-neutral-500 transition hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
    title="Join a space"
    onclick={onAdd}
  >
    +
  </button>
</nav>
