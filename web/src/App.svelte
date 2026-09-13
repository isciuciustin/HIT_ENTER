<script lang="ts">
  import { chat } from "./lib/chat.svelte";
  import ChannelList from "./components/ChannelList.svelte";
  import Composer from "./components/Composer.svelte";
  import HostDialog from "./components/HostDialog.svelte";
  import InviteDialog from "./components/InviteDialog.svelte";
  import JoinDialog from "./components/JoinDialog.svelte";
  import MemberList from "./components/MemberList.svelte";
  import MessagePane from "./components/MessagePane.svelte";
  import ServerRail from "./components/ServerRail.svelte";
  import SettingsDialog from "./components/SettingsDialog.svelte";
  import StatusDot from "./components/StatusDot.svelte";

  let ready = $state(false);
  let fatal = $state<string | null>(null);
  let showJoin = $state(false);
  let showInvite = $state(false);
  let showHost = $state(false);
  let showSettings = $state(false);
  let showMembers = $state(true);
  /** Prefills the join dialog when a link arrives from the desktop. */
  let joinWith = $state("");

  chat
    .init()
    .then(() => (ready = true))
    .catch((e) => (fatal = String(e)));

  // A `hitenter://` link from the OS opens the dialog and stops there. Joining
  // on arrival would make clicking a URL enough to enrol this device.
  $effect(() => {
    const link = chat.pendingLink;
    if (!link) return;
    joinWith = link;
    showJoin = true;
    chat.pendingLink = null;
  });

  // Coming back to the window means the messages on screen have now been
  // seen. Without this the badge for the channel you are looking at survives
  // until the next message arrives.
  $effect(() => {
    const onFocus = () => void chat.markRead();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  });

  function openJoin(prefill = "") {
    joinWith = prefill;
    showJoin = true;
  }
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
        You are not in any spaces yet. Join one with a link somebody sent you,
        or host your own — from this machine, with no router to configure.
      </p>
      <div class="mt-5 flex justify-center gap-2">
        <button
          onclick={() => openJoin()}
          class="rounded bg-[var(--color-accent)] px-4 py-2 text-sm font-semibold text-black"
        >
          join a space
        </button>
        <button
          onclick={() => (showHost = true)}
          class="rounded border border-[var(--color-edge)] px-4 py-2 text-sm text-neutral-300 transition hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
        >
          {chat.host?.space_exists ? "your space" : "host a space"}
        </button>
      </div>
      <button
        onclick={() => (showSettings = true)}
        class="mt-6 break-all font-mono text-[11px] text-neutral-700 transition hover:text-neutral-500"
        title="Network settings"
      >
        this device: {chat.deviceId}
      </button>
    </div>
  </main>
{:else}
  <div class="flex h-full">
    <ServerRail
      onAdd={() => openJoin()}
      onHost={() => (showHost = true)}
      onSettings={() => (showSettings = true)}
    />
    <ChannelList onInvite={() => (showInvite = true)} />

    <main class="flex min-w-0 flex-1 flex-col">
      <header
        class="flex items-center justify-between border-b border-[var(--color-edge)] px-4 py-2.5"
      >
        <h1 class="truncate text-sm font-semibold">
          <span class="text-neutral-600">#</span>{chat.activeChannelName}
        </h1>
        <div class="flex items-center gap-3 text-xs text-neutral-500">
          <button
            onclick={() => (showMembers = !showMembers)}
            class="rounded border border-[var(--color-edge)] px-2 py-0.5 text-[11px] transition hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]
              {showMembers ? 'text-neutral-300' : 'text-neutral-500'}"
            title="Show or hide the member list"
          >
            {chat.activeStatus === "offline"
              ? chat.members.length
              : `${chat.onlineCount}/${chat.members.length}`} members
          </button>
          {#if chat.activeIsOwnSpace}
            <button
              onclick={() => (showHost = true)}
              class="rounded border border-[var(--color-edge)] px-2 py-0.5 text-[11px] text-neutral-400 transition hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
              title="You host this space from this machine"
            >
              hosted here
            </button>
          {/if}
          <StatusDot status={chat.activeStatus} label />
        </div>
      </header>

      <MessagePane />
      <Composer />
    </main>

    {#if showMembers}
      <MemberList onClose={() => (showMembers = false)} />
    {/if}
  </div>
{/if}

{#if showJoin}
  <JoinDialog
    initial={joinWith}
    onClose={() => {
      showJoin = false;
      joinWith = "";
    }}
  />
{/if}
{#if showInvite}
  <InviteDialog onClose={() => (showInvite = false)} />
{/if}
{#if showHost}
  <HostDialog onClose={() => (showHost = false)} />
{/if}
{#if showSettings}
  <SettingsDialog onClose={() => (showSettings = false)} />
{/if}
