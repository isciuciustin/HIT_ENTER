<script lang="ts">
  import type { ConnectionStatus } from "../lib/api";

  // PLAN §6: a relayed connection is *working*, just slower. Showing it as an
  // error state would teach the user the app is broken when their ISP is the
  // thing that is unusual.
  let { status, label = false }: { status: ConnectionStatus; label?: boolean } =
    $props();

  const colour: Record<ConnectionStatus, string> = {
    direct: "bg-[var(--color-accent)]",
    relayed: "bg-amber-400",
    connecting: "bg-sky-400 animate-pulse",
    offline: "bg-neutral-600",
  };

  const explain: Record<ConnectionStatus, string> = {
    direct: "Direct — peer to peer, full speed.",
    relayed:
      "Relayed — your network would not allow a direct path, so traffic goes through a relay. Slower, still encrypted: a relay cannot read your messages.",
    connecting: "Connecting…",
    offline: "Offline — reading from your local copy.",
  };
</script>

<span class="inline-flex items-center gap-2" title={explain[status]}>
  <span class="h-2 w-2 shrink-0 rounded-full {colour[status]}"></span>
  {#if label}
    <span class="text-xs text-neutral-400">{status}</span>
  {/if}
</span>
