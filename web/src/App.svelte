<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  type AppInfo = {
    version: string;
    protocol: string;
    protocol_version: number;
  };

  // Svelte 5 runes. `$state` for reactive locals, no `let` reactivity magic.
  let info = $state<AppInfo | null>(null);
  let error = $state<string | null>(null);

  // Proves the frontend <-> Rust bridge is wired, which is M0's real test.
  invoke<AppInfo>("app_info")
    .then((v) => (info = v))
    .catch((e) => (error = String(e)));
</script>

<main class="flex h-full flex-col items-center justify-center gap-6 px-8 text-center">
  <h1 class="text-5xl font-bold tracking-tight">
    HIT<span class="text-[var(--color-accent)]">_</span>ENTER
  </h1>

  <p class="max-w-md text-sm text-neutral-400">
    Self-hosted, local-first group chat. Your server, your disk, your messages.
  </p>

  <div
    class="rounded-lg border border-[var(--color-edge)] bg-[var(--color-panel)] px-5 py-4 text-left text-xs"
  >
    {#if error}
      <p class="text-red-400">bridge error: {error}</p>
    {:else if info}
      <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
        <dt class="text-neutral-500">version</dt>
        <dd>{info.version}</dd>
        <dt class="text-neutral-500">protocol</dt>
        <dd>{info.protocol}</dd>
        <dt class="text-neutral-500">rev</dt>
        <dd>{info.protocol_version}</dd>
      </dl>
    {:else}
      <p class="text-neutral-500">connecting to backend…</p>
    {/if}
  </div>

  <p class="text-xs text-neutral-600">
    M0 skeleton — see <span class="text-neutral-500">docs/PLAN.md</span>
  </p>
</main>
