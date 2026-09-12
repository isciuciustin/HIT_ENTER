<script lang="ts">
  // Relays and discovery — the sovereignty ladder, as a form (PLAN §4).
  //
  // The pitch is "nobody can take this away from you", so every rung has to be
  // reachable from here without our involvement: n0's relays are a documented,
  // replaceable convenience and this pane says so, points at the alternatives,
  // and never pretends the default is the only option.

  import { api, asError, type NetworkConfig, type Relays } from "../lib/api";
  import { chat } from "../lib/chat.svelte";
  import Dialog from "./Dialog.svelte";

  let { onClose }: { onClose: () => void } = $props();

  const view = $derived(chat.settings);

  // A local copy: the form is a draft until it is saved, and a half-typed
  // relay URL must not become the running configuration.
  let mode = $state<Relays["mode"]>("n0");
  let customUrls = $state("");
  let n0Discovery = $state(true);
  let mdns = $state(true);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let note = $state<string | null>(null);
  let loaded = $state(false);

  $effect(() => {
    if (loaded || !view) return;
    loaded = true;
    mode = view.network.relays.mode;
    customUrls =
      view.network.relays.mode === "custom"
        ? view.network.relays.urls.join("\n")
        : "";
    n0Discovery = view.network.n0_discovery;
    mdns = view.network.mdns_discovery;
  });

  function draft(): NetworkConfig {
    const urls = customUrls
      .split(/\s+/)
      .map((u) => u.trim())
      .filter(Boolean);
    const relays: Relays =
      mode === "custom"
        ? { mode: "custom", urls }
        : mode === "disabled"
          ? { mode: "disabled" }
          : { mode: "n0" };
    return { relays, n0_discovery: n0Discovery, mdns_discovery: mdns };
  }

  async function save() {
    if (busy) return;
    busy = true;
    error = null;
    note = null;
    try {
      const saved = await api.setNetworkSettings(draft());
      chat.settings = saved.settings;
      note = saved.restart_required
        ? saved.host_restarted
          ? "Your space was reopened with the new settings. Your own connections keep the old ones until you restart the app."
          : "Saved. Your connections keep the old settings until you restart the app."
        : "Nothing to change.";
    } catch (e) {
      error = asError(e).message;
    } finally {
      busy = false;
    }
  }
</script>

<Dialog {onClose} wide>
  <h2 class="text-base font-semibold">Network</h2>
  <p class="mt-1 text-xs text-neutral-500">
    How this app finds other machines, and what it falls back to when a
    direct connection cannot be made. None of it is run by us.
  </p>

  {#if view}
    <p class="mt-3 text-xs text-neutral-400">
      Now: <span class="text-neutral-200">{view.summary}</span>
      {#if view.self_contained}
        <span class="ml-1 text-[var(--color-accent)]"
          >· nothing outside your own infrastructure</span
        >
      {/if}
    </p>
  {/if}

  <fieldset class="mt-4">
    <legend class="text-xs text-neutral-400">relays</legend>
    <p class="mt-1 text-[11px] text-neutral-600">
      A relay forwards bytes it cannot read. Traffic through one is still
      encrypted end to end — what a relay operator sees is who talks to whom,
      and when.
    </p>
    <div class="mt-2 space-y-1.5">
      <label class="flex items-start gap-2 text-xs text-neutral-300">
        <input type="radio" bind:group={mode} value="n0" class="mt-0.5" />
        <span>
          n0's public relays
          <span class="block text-[11px] text-neutral-600">
            Free, rate-limited, and documented by n0 as having no uptime
            guarantee. The default because it needs no configuration.
          </span>
        </span>
      </label>
      <label class="flex items-start gap-2 text-xs text-neutral-300">
        <input type="radio" bind:group={mode} value="custom" class="mt-0.5" />
        <span>
          your own relay
          <span class="block text-[11px] text-neutral-600">
            The relay binary is open source and lives in the iroh repository.
            One URL per line.
          </span>
        </span>
      </label>
      {#if mode === "custom"}
        <textarea
          bind:value={customUrls}
          rows="2"
          spellcheck="false"
          placeholder="https://relay.example.com"
          class="ml-6 w-[calc(100%-1.5rem)] rounded border border-[var(--color-edge)] bg-black/30 px-2 py-1.5 font-mono text-[11px] text-neutral-100 focus:border-[var(--color-accent)] focus:outline-none"
        ></textarea>
      {/if}
      <label class="flex items-start gap-2 text-xs text-neutral-300">
        <input type="radio" bind:group={mode} value="disabled" class="mt-0.5" />
        <span>
          none
          <span class="block text-[11px] text-neutral-600">
            Peers that cannot be reached directly stay unreachable. On a
            local network, with the option below, that is fine.
          </span>
        </span>
      </label>
    </div>
  </fieldset>

  <fieldset class="mt-4">
    <legend class="text-xs text-neutral-400">discovery</legend>
    <p class="mt-1 text-[11px] text-neutral-600">
      How a public key becomes an address when a link's hints have gone
      stale.
    </p>
    <div class="mt-2 space-y-1.5">
      <label class="flex items-start gap-2 text-xs text-neutral-300">
        <input type="checkbox" bind:checked={n0Discovery} class="mt-0.5" />
        <span>
          n0's DNS
          <span class="block text-[11px] text-neutral-600">
            Publishes this machine's address under its public key, and
            resolves others. Operated by n0.
          </span>
        </span>
      </label>
      <label class="flex items-start gap-2 text-xs text-neutral-300">
        <input type="checkbox" bind:checked={mdns} class="mt-0.5" />
        <span>
          local network
          <span class="block text-[11px] text-neutral-600">
            Announces on this network and listens for others. Needs no
            infrastructure at all, and works with the internet unplugged.
          </span>
        </span>
      </label>
    </div>
  </fieldset>

  {#if note}
    <p
      class="mt-4 rounded border border-[var(--color-edge)] bg-black/30 px-2 py-1.5 text-xs text-neutral-400"
    >
      {note}
    </p>
  {/if}
  {#if error}
    <p
      class="mt-4 rounded border border-red-500/40 bg-red-500/10 px-2 py-1.5 text-xs text-red-300"
    >
      {error}
    </p>
  {/if}

  <div class="mt-4 rounded border border-[var(--color-edge)] bg-black/20 p-3">
    <div class="text-[11px] uppercase tracking-wide text-neutral-600">
      this device
    </div>
    <div class="mt-1 break-all font-mono text-[11px] text-neutral-400">
      {chat.deviceId}
    </div>
    <p class="mt-1 text-[11px] text-neutral-600">
      What a space owner sees in their device list, and what they revoke by.
      It is not your account and it is not any space's address.
    </p>
    {#if view}
      <div
        class="mt-2 text-[11px] uppercase tracking-wide text-neutral-600"
      >
        your data
      </div>
      <div class="mt-1 break-all font-mono text-[11px] text-neutral-400">
        {view.data_dir}
      </div>
      <p class="mt-1 text-[11px] text-neutral-600">
        Every message you have seen, in plaintext, in an ordinary SQLite
        file. These settings are the <span class="font-mono">settings.json</span>
        next to it, and editing it by hand is a supported way to use this.
      </p>
    {/if}
  </div>

  <div class="mt-4 flex justify-end gap-2">
    <button
      onclick={onClose}
      class="rounded px-3 py-1.5 text-xs text-neutral-400 transition hover:bg-white/5"
    >
      close
    </button>
    <button
      onclick={save}
      disabled={busy}
      class="rounded bg-[var(--color-accent)] px-3 py-1.5 text-xs font-semibold text-black transition disabled:bg-neutral-800 disabled:text-neutral-600"
    >
      {busy ? "saving…" : "save"}
    </button>
  </div>
</Dialog>
