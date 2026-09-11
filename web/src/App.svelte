<script lang="ts">
  import { chat } from "./lib/chat.svelte";
  import ChannelList from "./components/ChannelList.svelte";
  import Composer from "./components/Composer.svelte";
  import InviteDialog from "./components/InviteDialog.svelte";
  import JoinDialog from "./components/JoinDialog.svelte";
  import MessagePane from "./components/MessagePane.svelte";
  import ServerRail from "./components/ServerRail.svelte";
  import StatusDot from "./components/StatusDot.svelte";

  let ready = $state(false);
  let fatal = $state<string | null>(null);
  let showJoin = $state(false);
  let showInvite = $state(false);

  chat
    .init()
    .then(() => (ready = true))
    .catch((e) => (fatal = String(e)));
</script>

{#if fatal}
  <main class="grid h-full place-items-center px-8 text-center">
    <div>
      <h1 class="text-2xl font-bold">HIT<span class="text-[var(--color-accent)]">_</span>ENTER</h1>
      <p class="mt-3 max-w-md text-sm text-red-400">{fatal}</p>
    </div>
  </main>
{:else if !ready}
  <main class="grid h-full place-items-center">
    <p class="text-sm text-neutral-600">opening your mirror…</p>
  </main>
{:else if chat.servers.length === 0}
  <!-- Nothing to show is a real state, not an empty grid: a brand new install
       has no spaces and needs one instruction, not four empty panels. -->
  <main class="grid h-full place-items-center px-8 text-center">
    <div class="max-w-sm">
      <h1 class="text-4xl font-bold tracking-tight">
        HIT<span class="text-[var(--color-accent)]">_</span>ENTER
      </h1>
      <p class="mt-3 text-sm text-neutral-400">
        You are not in any spaces yet. Join one with an address and an invite
        code, or run <span class="text-neutral-300">he-serverd</span> to host your own.
      </p>
      <button
        onclick={() => (showJoin = true)}
        class="mt-5 rounded bg-[var(--color-accent)] px-4 py-2 text-sm font-semibold text-black"
      >
        join a space
      </button>
      <p class="mt-6 break-all font-mono text-[11px] text-neutral-700">
        this device: {chat.deviceId}
      </p>
    </div>
  </main>
{:else}
  <div class="flex h-full">
    <ServerRail onAdd={() => (showJoin = true)} />
    <ChannelList onInvite={() => (showInvite = true)} />

    <main class="flex min-w-0 flex-1 flex-col">
      <header
        class="flex items-center justify-between border-b border-[var(--color-edge)] px-4 py-2.5"
      >
        <h1 class="truncate text-sm font-semibold">
          <span class="text-neutral-600">#</span>{chat.activeChannelName}
        </h1>
        <div class="flex items-center gap-3 text-xs text-neutral-500">
          <StatusDot status={chat.activeStatus} label />
        </div>
      </header>

      <MessagePane />
      <Composer />
    </main>
  </div>
{/if}

{#if showJoin}
  <JoinDialog onClose={() => (showJoin = false)} />
{/if}
{#if showInvite}
  <InviteDialog onClose={() => (showInvite = false)} />
{/if}
