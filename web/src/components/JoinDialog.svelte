<script lang="ts">
  import { api, asError, type ParsedLink } from "../lib/api";
  import { chat } from "../lib/chat.svelte";
  import Dialog from "./Dialog.svelte";

  let { onClose, initial = "" }: { onClose: () => void; initial?: string } =
    $props();

  let address = $state("");
  let username = $state("");
  let password = $state("");
  let invite = $state("");
  let busy = $state(false);

  // A link can arrive from the desktop while this dialog is already open, so
  // `initial` is watched rather than read once — but only when it actually
  // changes, or every keystroke would be overwritten by the prefill.
  let seeded = $state<string | null>(null);
  $effect(() => {
    if (initial && initial !== seeded) {
      seeded = initial;
      address = initial;
    }
  });

  /** What the pasted string turned out to be. Parsed in Rust, by the same
      code that parses a link arriving from the desktop (PLAN §5). */
  let parsed = $state<ParsedLink | null>(null);
  let parseError = $state<string | null>(null);

  // No invite means "I already have an account here and this is a new
  // machine" — the password enrols the device. With one it also creates the
  // account. Either way the password is used once and never stored (PLAN §3).
  const valid = $derived(
    address.trim().length > 0 &&
      username.trim().length >= chat.limits.username_min_chars &&
      password.length >= chat.limits.password_min_bytes,
  );

  async function reparse() {
    const text = address.trim();
    parsed = null;
    parseError = null;
    if (!text) return;
    try {
      const link = await api.parseLink(text);
      parsed = link;
      // A link carries its own code; filling the field in shows the user what
      // is about to be used rather than hiding it inside the paste.
      if (link.code && !invite.trim()) invite = link.code;
    } catch (e) {
      parseError = asError(e).message;
    }
  }

  // Parse whatever is in the box, including a link handed over by the desktop.
  // Debounced: this crosses the bridge and reads the mirror, and a link is
  // pasted rather than typed, so there is nothing to gain from doing it once
  // per keystroke.
  $effect(() => {
    void address;
    const timer = setTimeout(reparse, 150);
    return () => clearTimeout(timer);
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    busy = true;
    const ok = await chat.join({
      address: address.trim(),
      username: username.trim(),
      password,
      invite: invite.trim() || undefined,
    });
    busy = false;
    // The password leaves this component the moment it is used.
    password = "";
    if (ok) onClose();
  }
</script>

<Dialog {onClose}>
  <form onsubmit={submit}>
    <h2 class="text-base font-semibold">Join a space</h2>
    <p class="mt-1 text-xs text-neutral-500">
      Paste the whole <span class="font-mono text-neutral-400">hitenter://</span>
      link somebody sent you. A bare address works too, with the invite code
      below.
    </p>

    <label class="mt-4 block text-xs text-neutral-400">
      link or address
      <textarea
        bind:value={address}
        rows="2"
        spellcheck="false"
        autocomplete="off"
        placeholder="hitenter://join?t=…&c=…"
        class="mt-1 w-full resize-none rounded border border-[var(--color-edge)] bg-black/30 px-2 py-1.5 font-mono text-[11px] text-neutral-100 focus:border-[var(--color-accent)] focus:outline-none"
      ></textarea>
    </label>

    {#if parsed}
      <div
        class="mt-1 rounded border border-[var(--color-edge)] bg-black/20 px-2 py-1.5 text-[11px] text-neutral-500"
      >
        <div class="break-all">
          space <span class="font-mono text-neutral-400">{parsed.endpoint_id}</span>
        </div>
        {#if parsed.is_own_space}
          <div class="mt-0.5 text-[var(--color-accent)]">
            That is the space this machine hosts. Joining it still goes over
            the network like anyone else's would.
          </div>
        {:else if parsed.known}
          <div class="mt-0.5 text-amber-300">
            You are already in this space. Joining again re-enrols this device.
          </div>
        {/if}
        {#if !parsed.code}
          <div class="mt-0.5">
            No invite code in this link — fine if you already have an account
            here.
          </div>
        {/if}
      </div>
    {:else if parseError && address.trim()}
      <p class="mt-1 text-[11px] text-red-400">{parseError}</p>
    {/if}

    <label class="mt-3 block text-xs text-neutral-400">
      invite code
      <span class="text-neutral-600"
        >— leave empty if you already have an account here</span
      >
      <input
        bind:value={invite}
        spellcheck="false"
        autocomplete="off"
        placeholder="K7QP-2M4X-9WTZ"
        class="mt-1 w-full rounded border border-[var(--color-edge)] bg-black/30 px-2 py-1.5 font-mono text-xs uppercase text-neutral-100 focus:border-[var(--color-accent)] focus:outline-none"
      />
    </label>

    <div class="mt-3 grid grid-cols-2 gap-3">
      <label class="block text-xs text-neutral-400">
        username
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
          autocomplete="current-password"
          class="mt-1 w-full rounded border border-[var(--color-edge)] bg-black/30 px-2 py-1.5 text-sm text-neutral-100 focus:border-[var(--color-accent)] focus:outline-none"
        />
      </label>
    </div>

    <p class="mt-2 text-[11px] text-neutral-600">
      Accounts are per-space. Your password here is not your password anywhere
      else, and it is hashed on the host — nobody can read it back. The person
      hosting this space can read every message in it, so join spaces hosted by
      people you trust.
    </p>

    {#if chat.error}
      <p class="mt-3 rounded border border-red-500/40 bg-red-500/10 px-2 py-1.5 text-xs text-red-300">
        {#if chat.error.code === "INVITE_INVALID"}
          That invite code is not valid — it may be used up or expired.
        {:else if chat.error.code === "BAD_CREDENTIALS"}
          Wrong username or password for that space.
        {:else if chat.error.code === "USERNAME_TAKEN"}
          That username is taken on this space.
        {:else if chat.error.code === "RATE_LIMITED"}
          Too many attempts. Try again in {chat.error.retry_after ?? 30}s.
        {:else if chat.error.code === "DEVICE_NOT_ENROLLED"}
          This device is not enrolled here yet — enter your password to enrol it.
        {:else}
          {chat.error.message}
        {/if}
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
        {busy ? "joining…" : "join"}
      </button>
    </div>
  </form>
</Dialog>
