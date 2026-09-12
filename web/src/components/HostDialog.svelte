<script lang="ts">
  // Hosting, as a switch.
  //
  // The thing this panel has to teach, and the reason for its wording: **the
  // person hosting a space can read every message in it** (PLAN §10). That is
  // the trust model, it is deliberate, and the moment somebody decides to host
  // is the moment to say it plainly rather than bury it in a document.
  //
  // The second thing: the space's address is not this device's address. They
  // are separate keys with separate lifetimes (PLAN §3), and conflating them
  // in the UI is how a user ends up handing out the wrong one.

  import { copyText } from "../lib/api";
  import { chat } from "../lib/chat.svelte";
  import Dialog from "./Dialog.svelte";
  import StatusDot from "./StatusDot.svelte";

  let { onClose }: { onClose: () => void } = $props();

  const host = $derived(chat.host);

  let name = $state("");
  let username = $state("");
  let password = $state("");
  let busy = $state(false);
  let copied = $state<string | null>(null);

  const valid = $derived(
    name.trim().length > 0 &&
      username.trim().length >= chat.limits.username_min_chars &&
      password.length >= chat.limits.password_min_bytes,
  );

  async function create(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    busy = true;
    const ok = await chat.createSpace({
      name: name.trim(),
      username: username.trim(),
      password,
    });
    busy = false;
    // The password leaves this component the moment it is used. It enrolled
    // this device; the device key is the login from here on (PLAN §3).
    password = "";
    if (ok) onClose();
  }

  async function toggle(running: boolean) {
    if (busy) return;
    busy = true;
    await chat.setHosting(running);
    busy = false;
  }

  async function copy(what: string, value: string) {
    if (!(await copyText(value))) return;
    copied = what;
    setTimeout(() => (copied = null), 1500);
  }
</script>

<Dialog {onClose} wide>
  {#if host?.space_exists}
    <div class="flex items-start justify-between gap-3">
      <div class="min-w-0">
        <h2 class="truncate text-base font-semibold">{host.space_name}</h2>
        <p class="mt-1 text-xs text-neutral-500">
          Hosted on this machine. No port was forwarded and no certificate
          was obtained — the address is a public key.
        </p>
      </div>
      <StatusDot status={host.running ? "direct" : "offline"} />
    </div>

    <div
      class="mt-4 flex items-center justify-between rounded border border-[var(--color-edge)] bg-black/30 px-3 py-2.5"
    >
      <div class="text-sm">
        {host.running ? "open" : "closed"}
        <span class="ml-2 text-xs text-neutral-600">
          {host.running
            ? "members can connect"
            : "the database and its identity are untouched"}
        </span>
      </div>
      <button
        onclick={() => toggle(!host.running)}
        disabled={busy}
        class="rounded px-3 py-1.5 text-xs font-semibold transition disabled:opacity-40
          {host.running
          ? 'border border-[var(--color-edge)] text-neutral-300 hover:border-red-500/60 hover:text-red-400'
          : 'bg-[var(--color-accent)] text-black'}"
      >
        {host.running ? "close space" : "open space"}
      </button>
    </div>

    {#if host.running && !host.reachable_remotely}
      <p
        class="mt-3 rounded border border-amber-500/40 bg-amber-500/10 px-2 py-1.5 text-xs text-amber-300"
      >
        No relay yet — right now this space is reachable on your local
        network and nowhere else. It usually settles within a few seconds;
        if it does not, check the relay setting.
      </p>
    {/if}

    {#if host.endpoint_id}
      <div class="mt-4 space-y-3">
        <div>
          <div class="text-[11px] uppercase tracking-wide text-neutral-600">
            space address
          </div>
          <div class="break-all font-mono text-[11px] text-neutral-400">
            {host.endpoint_id}
          </div>
          <p class="mt-1 text-[11px] text-neutral-600">
            This is the space's key, not this device's. It survives your IP
            changing, your ISP changing, and this laptop moving city.
          </p>
        </div>

        {#if host.link}
          <button
            onclick={() => copy("link", host.link!)}
            class="w-full rounded border border-[var(--color-edge)] px-3 py-1.5 text-xs text-neutral-300 transition hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
          >
            {copied === "link"
              ? "copied"
              : "copy this space's address as a link"}
          </button>
          <p class="text-[11px] text-neutral-600">
            An address with no invite code: enough to point a second machine
            of your own at this space, not enough for anyone to register.
            Use <span class="text-neutral-400">invite</span> for that.
          </p>
        {/if}
      </div>
    {/if}

    <div class="mt-4 rounded border border-[var(--color-edge)] bg-black/20 p-3">
      <div class="text-[11px] uppercase tracking-wide text-neutral-600">
        back this up
      </div>
      <div class="mt-1 break-all font-mono text-[11px] text-neutral-400">
        {host.data_dir}
      </div>
      <p class="mt-1 text-[11px] text-neutral-600">
        <span class="text-neutral-400">server.db</span> holds the space's
        secret key. Lose it and every invite ever issued is dead and the
        address changes. Treat it like an SSH host key.
      </p>
    </div>
  {:else}
    <h2 class="text-base font-semibold">Host a space</h2>
    <p class="mt-1 text-xs text-neutral-500">
      Your machine becomes a space others can join — from anywhere, with no
      router configuration. It runs while the app does.
    </p>

    <form onsubmit={create}>
      <label class="mt-4 block text-xs text-neutral-400">
        space name
        <input
          bind:value={name}
          maxlength={chat.limits.channel_name_max_chars}
          placeholder="Kitchen Table"
          class="mt-1 w-full rounded border border-[var(--color-edge)] bg-black/30 px-2 py-1.5 text-sm text-neutral-100 focus:border-[var(--color-accent)] focus:outline-none"
        />
      </label>

      <div class="mt-3 grid grid-cols-2 gap-3">
        <label class="block text-xs text-neutral-400">
          your username here
          <input
            bind:value={username}
            spellcheck="false"
            autocomplete="off"
            maxlength={chat.limits.username_max_chars}
            class="mt-1 w-full rounded border border-[var(--color-edge)] bg-black/30 px-2 py-1.5 text-sm text-neutral-100 focus:border-[var(--color-accent)] focus:outline-none"
          />
        </label>
        <label class="block text-xs text-neutral-400">
          password
          <input
            bind:value={password}
            type="password"
            autocomplete="new-password"
            class="mt-1 w-full rounded border border-[var(--color-edge)] bg-black/30 px-2 py-1.5 text-sm text-neutral-100 focus:border-[var(--color-accent)] focus:outline-none"
          />
        </label>
      </div>

      <p class="mt-2 text-[11px] text-neutral-600">
        You are the owner. This password enrols this machine once and is then
        never needed again here — it is hashed and cannot be read back, not
        even by you. There is no reset: you are the recovery mechanism.
      </p>

      <div
        class="mt-4 rounded border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-[11px] text-amber-200/90"
      >
        <strong class="font-semibold">Messages are stored in plaintext.</strong>
        Anyone with this machine's disk — which is you — can read every
        message in the space. That is deliberate: it is what makes offline
        search, moderation and <span class="font-mono">cp server.db</span>
        backups work. Traffic is still encrypted end to end in transit, so
        your ISP and any relay see nothing.
      </div>

      {#if chat.error}
        <p
          class="mt-3 rounded border border-red-500/40 bg-red-500/10 px-2 py-1.5 text-xs text-red-300"
        >
          {chat.error.message}
        </p>
      {/if}

      <div class="mt-4 flex justify-end gap-2">
        <button
          type="button"
          onclick={onClose}
          class="rounded px-3 py-1.5 text-xs text-neutral-400 transition hover:bg-white/5"
        >
          cancel
        </button>
        <button
          type="submit"
          disabled={!valid || busy}
          class="rounded bg-[var(--color-accent)] px-3 py-1.5 text-xs font-semibold text-black transition disabled:bg-neutral-800 disabled:text-neutral-600"
        >
          {busy ? "opening…" : "create and open"}
        </button>
      </div>
    </form>
  {/if}

{#if host?.space_exists}
  <div class="mt-4 flex justify-end">
    <button
      onclick={onClose}
      class="rounded px-3 py-1.5 text-xs text-neutral-400 transition hover:bg-white/5"
    >
      close
    </button>
  </div>
{/if}
</Dialog>
