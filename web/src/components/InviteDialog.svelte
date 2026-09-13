<script lang="ts">
  import {
    api,
    asError,
    copyText,
    type Invite,
    type InviteInfo,
  } from "../lib/api";
  import { chat } from "../lib/chat.svelte";
  import Dialog from "./Dialog.svelte";

  let { onClose }: { onClose: () => void } = $props();

  let invite = $state<Invite | null>(null);
  let error = $state<string | null>(null);
  let maxUses = $state(1);
  let busy = $state(false);
  let copied = $state<string | null>(null);

  /** Existing codes, for the owner to revoke. Nobody else may list them. */
  let existing = $state<InviteInfo[]>([]);
  let confirming = $state<string | null>(null);

  $effect(() => {
    if (chat.amOwner) void refresh();
  });

  async function refresh() {
    existing = await chat.listInvites();
  }

  async function revoke(code: string) {
    if (confirming !== code) {
      confirming = code;
      return;
    }
    confirming = null;
    if (await chat.revokeInvite(code)) await refresh();
  }

  /** `∞` reads better than "unlimited" in a column two characters wide. */
  function uses(invite: InviteInfo): string {
    return `${invite.uses}/${invite.max_uses ?? "∞"}`;
  }

  function expired(invite: InviteInfo): boolean {
    return invite.expires_at != null && invite.expires_at * 1000 < Date.now();
  }

  function exhausted(invite: InviteInfo): boolean {
    return invite.max_uses != null && invite.uses >= invite.max_uses;
  }

  async function mint() {
    if (!chat.activeServer || busy) return;
    busy = true;
    error = null;
    try {
      invite = await api.createInvite({
        endpointId: chat.activeServer,
        maxUses,
      });
    } catch (e) {
      error = asError(e).message;
    } finally {
      busy = false;
    }
  }

  async function copy(what: "link" | "code", value: string) {
    if (!(await copyText(value))) {
      error = "could not reach the clipboard — select the text above instead";
      return;
    }
    copied = what;
    setTimeout(() => (copied = null), 1500);
  }
</script>

<Dialog {onClose}>
  <h2 class="text-base font-semibold">Invite someone</h2>
  <p class="mt-1 text-xs text-neutral-500">
    One link carries both halves: where the space is, and permission to make
    an account. Registration is invite-gated, so the code is what keeps
    strangers out.
  </p>

  {#if invite}
    <div class="mt-4 rounded border border-[var(--color-edge)] bg-black/30 p-3">
      <div class="text-[11px] uppercase tracking-wide text-neutral-600">
        link
      </div>
      <div class="mt-0.5 break-all font-mono text-[11px] text-neutral-300">
        {invite.link}
      </div>
      <div class="mt-2 text-[11px] uppercase tracking-wide text-neutral-600">
        code
      </div>
      <div class="font-mono text-lg text-[var(--color-accent)]">
        {invite.code}
      </div>
    </div>

    <div class="mt-3 grid grid-cols-2 gap-2">
      <button
        onclick={() => copy("link", invite!.link)}
        class="rounded bg-[var(--color-accent)] px-3 py-1.5 text-xs font-semibold text-black transition"
      >
        {copied === "link" ? "copied" : "copy link"}
      </button>
      <button
        onclick={() => copy("code", invite!.code)}
        class="rounded border border-[var(--color-edge)] px-3 py-1.5 text-xs text-neutral-300 transition hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
      >
        {copied === "code" ? "copied" : "copy code only"}
      </button>
    </div>

    <p class="mt-2 text-[11px] text-neutral-600">
      Paste the link into a chat message. The address inside it is a public
      key, so a link that has been tampered with points at nothing rather
      than at somebody else — there is nothing for either of you to verify.
    </p>

    {#if chat.activeIsOwnSpace && !invite.reachable_remotely}
      <p
        class="mt-3 rounded border border-amber-500/40 bg-amber-500/10 px-2 py-1.5 text-xs text-amber-300"
      >
        Your space has not reached a relay yet, so this link will only work
        on your local network. Wait a moment and mint another one.
      </p>
    {/if}
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

  {#if chat.amOwner && existing.length > 0}
    <h3 class="mt-6 text-[11px] uppercase tracking-wide text-neutral-500">
      codes that already exist
    </h3>
    <p class="mt-1 text-[11px] text-neutral-600">
      Revoking one kills the code. The accounts already made with it stay —
      they are members now, and an invite is a door, not a lease.
    </p>
    <ul class="mt-2 space-y-1">
      {#each existing as invite (invite.code)}
        {@const dead = expired(invite) || exhausted(invite)}
        <li
          class="flex items-center gap-2 rounded border border-[var(--color-edge)] px-2 py-1.5 text-xs
            {dead ? 'opacity-50' : ''}"
        >
          <span class="min-w-0 flex-1 truncate font-mono text-neutral-300">
            {invite.code}
          </span>
          <span class="shrink-0 text-[11px] text-neutral-600" title="uses">
            {#if expired(invite)}
              expired
            {:else}
              {uses(invite)}
            {/if}
          </span>
          <button
            onclick={() => revoke(invite.code)}
            class="shrink-0 rounded px-1.5 py-0.5 text-[11px] transition
              {confirming === invite.code
              ? 'bg-red-500/20 text-red-300'
              : 'text-neutral-500 hover:text-red-400'}"
          >
            {confirming === invite.code ? "really" : "revoke"}
          </button>
        </li>
      {/each}
    </ul>
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
</Dialog>
