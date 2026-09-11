<script lang="ts">
  import { api, asError } from "../lib/api";
  import { chat } from "../lib/chat.svelte";

  let { onClose }: { onClose: () => void } = $props();

  let code = $state<string | null>(null);
  let error = $state<string | null>(null);
  let maxUses = $state(1);
  let busy = $state(false);
  let copied = $state(false);

  async function mint() {
    if (!chat.activeServer || busy) return;
    busy = true;
    error = null;
    try {
      code = await api.createInvite({
        endpointId: chat.activeServer,
        maxUses,
      });
    } catch (e) {
      error = asError(e).message;
    } finally {
      busy = false;
    }
  }

  async function copy() {
    if (!code || !chat.activeServer) return;
    // Both halves are needed to join: the address and the code (PLAN §5).
    await navigator.clipboard.writeText(`${chat.activeServer} ${code}`);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }
</script>

<div class="fixed inset-0 z-50 grid place-items-center bg-black/70 p-6">
  <div
    class="w-full max-w-md rounded-xl border border-[var(--color-edge)] bg-[var(--color-panel)] p-5"
  >
    <h2 class="text-base font-semibold">Invite someone</h2>
    <p class="mt-1 text-xs text-neutral-500">
      A code plus this space's address is everything they need. Registration is
      invite-gated, so the code is what keeps strangers out.
    </p>

    {#if code}
      <div class="mt-4 rounded border border-[var(--color-edge)] bg-black/30 p-3">
        <div class="text-[11px] uppercase tracking-wide text-neutral-600">code</div>
        <div class="font-mono text-lg text-[var(--color-accent)]">{code}</div>
        <div class="mt-2 text-[11px] uppercase tracking-wide text-neutral-600">address</div>
        <div class="break-all font-mono text-[11px] text-neutral-400">
          {chat.activeServer}
        </div>
      </div>
      <button
        onclick={copy}
        class="mt-3 w-full rounded border border-[var(--color-edge)] px-3 py-1.5 text-xs text-neutral-300 transition hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
      >
        {copied ? "copied" : "copy address and code"}
      </button>
    {:else}
      <label class="mt-4 block text-xs text-neutral-400">
        how many people may use it
        <input
          type="number"
          min="1"
          bind:value={maxUses}
          class="mt-1 w-full rounded border border-[var(--color-edge)] bg-black/30 px-2 py-1.5 text-sm text-neutral-100 focus:border-[var(--color-accent)] focus:outline-none"
        />
      </label>
      <button
        onclick={mint}
        disabled={busy}
        class="mt-3 w-full rounded bg-[var(--color-accent)] px-3 py-1.5 text-xs font-semibold text-black transition disabled:bg-neutral-800 disabled:text-neutral-600"
      >
        {busy ? "minting…" : "create invite"}
      </button>
    {/if}

    {#if error}
      <p class="mt-3 rounded border border-red-500/40 bg-red-500/10 px-2 py-1.5 text-xs text-red-300">
        {error}
      </p>
    {/if}

    <div class="mt-4 flex justify-end">
      <button
        onclick={onClose}
        class="rounded px-3 py-1.5 text-xs text-neutral-400 transition hover:bg-white/5"
      >
        close
      </button>
    </div>
  </div>
</div>
