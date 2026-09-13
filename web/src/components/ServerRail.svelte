<script lang="ts">
  import { chat } from "../lib/chat.svelte";
  import StatusDot from "./StatusDot.svelte";

  let {
    onAdd,
    onHost,
    onSettings,
  }: { onAdd: () => void; onHost: () => void; onSettings: () => void } =
    $props();

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
      <!-- A count, not a dot: the rail is the only place a space you are not
           looking at can tell you anything, so it may as well say how much. -->
      {#if server.unread > 0 && chat.activeServer !== server.endpoint_id}
        <span
          class="absolute -right-1 -top-1 min-w-4 rounded-full bg-[var(--color-accent)] px-1 py-0.5 text-[10px] font-bold leading-none text-black"
          aria-label="{server.unread} unread"
        >
          {server.unread > 99 ? "99+" : server.unread}
        </span>
      {/if}
      <span class="absolute -bottom-0.5 -right-0.5">
        <StatusDot status={chat.status[server.endpoint_id] ?? "offline"} />
      </span>
      {#if chat.host?.running && chat.host.endpoint_id === server.endpoint_id}
        <!-- A space you host is a different relationship from one you joined:
             its database, its members and its secret key are on this disk. -->
        <span
          class="absolute -left-0.5 -top-0.5 text-[9px] text-[var(--color-accent)]"
          title="Hosted on this machine"
        >
          ●
        </span>
      {/if}
    </button>
  {/each}

  <button
    class="grid h-11 w-11 place-items-center rounded-2xl border border-dashed border-[var(--color-edge)] text-xl text-neutral-500 transition hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
    title="Join a space"
    onclick={onAdd}
  >
    +
  </button>

  <div class="mt-auto flex flex-col items-center gap-1">
    <button
      class="grid h-9 w-9 place-items-center rounded-xl text-lg transition hover:bg-white/5 hover:text-[var(--color-accent)]
        {chat.host?.running ? 'text-[var(--color-accent)]' : 'text-neutral-600'}"
      title={chat.host?.running
        ? "Your space is open"
        : chat.host?.space_exists
          ? "Your space is closed"
          : "Host a space"}
      onclick={onHost}
    >
      <!-- Drawn rather than typed: the obvious characters for these two are
           emoji in most desktop fonts, and a colour emoji ignores the state
           colour that is the whole point of the icon. -->
      <svg viewBox="0 0 16 16" class="h-4 w-4" aria-hidden="true">
        <path
          d="M2 7.5 8 2.5l6 5M3.5 6.8V13h9V6.8"
          fill="none"
          stroke="currentColor"
          stroke-width="1.4"
          stroke-linejoin="round"
        />
      </svg>
    </button>
    <button
      class="grid h-9 w-9 place-items-center rounded-xl text-neutral-600 transition hover:bg-white/5 hover:text-neutral-300"
      title="Network settings"
      onclick={onSettings}
    >
      <svg viewBox="0 0 16 16" class="h-4 w-4" aria-hidden="true">
        <circle
          cx="8"
          cy="8"
          r="2.4"
          fill="none"
          stroke="currentColor"
          stroke-width="1.4"
        />
        <path
          d="M8 1.4v2M8 12.6v2M1.4 8h2M12.6 8h2M3.3 3.3l1.4 1.4M11.3 11.3l1.4 1.4M12.7 3.3l-1.4 1.4M4.7 11.3l-1.4 1.4"
          stroke="currentColor"
          stroke-width="1.4"
          stroke-linecap="round"
        />
      </svg>
    </button>
  </div>
</nav>
