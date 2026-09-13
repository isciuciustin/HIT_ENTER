<script lang="ts">
  // One member, and what can be done about them.
  //
  // The three actions are deliberately not the same weight, and the dialog
  // says which is which rather than lining them up as equals:
  //
  //  - **revoke a device** logs one machine out. Narrow, and reversible with
  //    a password.
  //  - **kick** logs every machine out. Their password still works.
  //  - **ban** also refuses the password. Reversible from here.
  //
  // It doubles as "your devices" for yourself, because a member looking at
  // their own machines wants the same list the owner is looking at.

  import { chat } from "../lib/chat.svelte";
  import type { DeviceInfo, Member } from "../lib/api";
  import Dialog from "./Dialog.svelte";

  let { member, onClose }: { member: Member; onClose: () => void } = $props();

  let devices = $state<DeviceInfo[]>([]);
  let loading = $state(true);
  let busy = $state(false);
  /** Actions that take a second click, keyed by what is being confirmed. */
  let confirming = $state<string | null>(null);

  const isMe = $derived(member.id === chat.myId);
  /** The owner sees anybody's machines; everybody else only their own. */
  const maySeeDevices = $derived(chat.amOwner || isMe);
  /** You cannot kick or ban yourself, and nobody can do either to the owner. */
  const mayModerate = $derived(chat.amOwner && !isMe && !member.is_owner);

  $effect(() => {
    const id = member.id;
    if (!maySeeDevices) {
      loading = false;
      return;
    }
    void (async () => {
      loading = true;
      devices = await chat.devicesFor(id);
      loading = false;
    })();
  });

  async function act(key: string, run: () => Promise<boolean>) {
    if (confirming !== key) {
      confirming = key;
      return;
    }
    busy = true;
    const ok = await run();
    busy = false;
    confirming = null;
    if (ok && maySeeDevices) devices = await chat.devicesFor(member.id);
  }

  function when(seconds: number): string {
    return new Date(seconds * 1000).toLocaleString();
  }

  /** A public key is 52 characters of z-base-32 and nobody reads all of it. */
  function shortKey(key: string): string {
    return `${key.slice(0, 8)}…${key.slice(-6)}`;
  }
</script>

<Dialog {onClose} wide>
  <div class="flex items-baseline gap-2">
    <h2 class="text-lg font-semibold">
      {member.display_name ?? member.username}
    </h2>
    {#if member.is_owner}
      <span class="text-xs text-neutral-500">hosts this space</span>
    {:else if isMe}
      <span class="text-xs text-neutral-500">you</span>
    {/if}
    {#if member.banned}
      <span class="rounded bg-red-500/20 px-1.5 py-0.5 text-[11px] text-red-300">
        banned
      </span>
    {/if}
  </div>

  {#if maySeeDevices}
    <h3 class="mt-5 text-xs uppercase tracking-wide text-neutral-500">
      machines
    </h3>
    <p class="mt-1 text-xs text-neutral-600">
      Each one is a public key this space has enrolled. Revoking a key does not
      blacklist it — the password can enrol it again. What it takes away is the
      passwordless login.
    </p>

    {#if loading}
      <p class="mt-3 text-sm text-neutral-600">reading…</p>
    {:else if devices.length === 0}
      <p class="mt-3 text-sm text-neutral-600">no machines enrolled</p>
    {:else}
      <ul class="mt-3 space-y-1">
        {#each devices as device (device.endpoint_id)}
          <li
            class="flex items-center gap-3 rounded border border-[var(--color-edge)] px-3 py-2 text-sm
              {device.revoked_at ? 'opacity-50' : ''}"
          >
            <div class="min-w-0 flex-1">
              <div class="truncate font-mono text-xs" title={device.endpoint_id}>
                {shortKey(device.endpoint_id)}
                {#if device.current}
                  <span class="ml-1 not-italic text-[var(--color-accent)]">
                    this machine
                  </span>
                {/if}
              </div>
              <div class="text-[11px] text-neutral-600">
                {#if device.revoked_at}
                  revoked {when(device.revoked_at)}
                {:else}
                  last seen {when(device.last_seen)}
                {/if}
              </div>
            </div>
            {#if !device.revoked_at}
              <button
                onclick={() =>
                  act(`device:${device.endpoint_id}`, () =>
                    chat.revokeDevice(member.id, device.endpoint_id),
                  )}
                disabled={busy}
                class="shrink-0 rounded px-2 py-1 text-[11px] transition disabled:opacity-40
                  {confirming === `device:${device.endpoint_id}`
                  ? 'bg-red-500/20 text-red-300'
                  : 'text-neutral-500 hover:text-red-400'}"
              >
                {#if confirming === `device:${device.endpoint_id}`}
                  {device.current ? "logs you out — really?" : "really revoke"}
                {:else}
                  revoke
                {/if}
              </button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}

  {#if mayModerate}
    <h3 class="mt-6 text-xs uppercase tracking-wide text-neutral-500">
      membership
    </h3>
    <div class="mt-3 space-y-2">
      <div class="flex items-start gap-3">
        <button
          onclick={() => act("kick", () => chat.kickMember(member.id))}
          disabled={busy || member.banned}
          class="w-28 shrink-0 rounded border px-3 py-1.5 text-sm transition disabled:opacity-40
            {confirming === 'kick'
            ? 'border-amber-500/60 bg-amber-500/10 text-amber-300'
            : 'border-[var(--color-edge)] text-neutral-300 hover:border-amber-500/60'}"
        >
          {confirming === "kick" ? "really kick" : "kick"}
        </button>
        <p class="text-xs text-neutral-500">
          Logs them out of every machine. Their password still works, so they
          can come back — the proportionate answer to a laptop left in a pub.
        </p>
      </div>

      <div class="flex items-start gap-3">
        <button
          onclick={() =>
            act("ban", () => chat.setMemberBanned(member.id, !member.banned))}
          disabled={busy}
          class="w-28 shrink-0 rounded border px-3 py-1.5 text-sm transition disabled:opacity-40
            {confirming === 'ban'
            ? 'border-red-500/60 bg-red-500/10 text-red-300'
            : 'border-[var(--color-edge)] text-neutral-300 hover:border-red-500/60'}"
        >
          {#if member.banned}
            {confirming === "ban" ? "really un-ban" : "un-ban"}
          {:else}
            {confirming === "ban" ? "really ban" : "ban"}
          {/if}
        </button>
        <p class="text-xs text-neutral-500">
          {#if member.banned}
            Lets them log in again. Their account and everything they said were
            never deleted.
          {:else}
            Logs them out and refuses their password too. Reversible from here,
            and their messages stay.
          {/if}
        </p>
      </div>
    </div>
  {:else if chat.amOwner && member.is_owner}
    <p class="mt-6 text-xs text-neutral-600">
      You cannot kick or ban yourself. A space is administered through the
      owner's own client, so locking it out would leave nobody able to reach
      these tools.
    </p>
  {/if}

  {#if chat.error}
    <p class="mt-4 rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-sm text-red-300">
      {chat.error.message}
    </p>
  {/if}

  <div class="mt-5 flex justify-end">
    <button
      onclick={onClose}
      class="rounded px-3 py-1.5 text-sm text-neutral-400 transition hover:text-neutral-200"
    >
      close
    </button>
  </div>
</Dialog>
