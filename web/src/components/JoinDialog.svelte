<script lang="ts">
  import { chat } from "../lib/chat.svelte";

  let { onClose }: { onClose: () => void } = $props();

  let endpointId = $state("");
  let username = $state("");
  let password = $state("");
  let invite = $state("");
  let busy = $state(false);

  // No invite means "I already have an account here and this is a new
  // machine" — the password enrols the device. With one it also creates the
  // account. Either way the password is used once and never stored (PLAN §3).
  const valid = $derived(
    endpointId.trim().length > 0 &&
      username.trim().length >= chat.limits.username_min_chars &&
      password.length >= chat.limits.password_min_bytes,
  );

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    busy = true;
    const ok = await chat.join({
      endpointId: endpointId.trim(),
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

<div class="fixed inset-0 z-50 grid place-items-center bg-black/70 p-6">
  <form
    onsubmit={submit}
    class="w-full max-w-md rounded-xl border border-[var(--color-edge)] bg-[var(--color-panel)] p-5"
  >
    <h2 class="text-base font-semibold">Join a space</h2>
    <p class="mt-1 text-xs text-neutral-500">
      A space is addressed by its public key. Paste the one whoever is hosting
      gave you, along with their invite code.
    </p>

    <label class="mt-4 block text-xs text-neutral-400">
      space address
      <input
        bind:value={endpointId}
        spellcheck="false"
        autocomplete="off"
        placeholder="8ab551727584e9f1…"
        class="mt-1 w-full rounded border border-[var(--color-edge)] bg-black/30 px-2 py-1.5 font-mono text-xs text-neutral-100 focus:border-[var(--color-accent)] focus:outline-none"
      />
    </label>

    <label class="mt-3 block text-xs text-neutral-400">
      invite code <span class="text-neutral-600">— leave empty if you already have an account here</span>
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
      else, and it is hashed on the host — nobody can read it back.
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
</div>
