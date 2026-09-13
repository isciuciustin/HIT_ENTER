<script lang="ts">
  import { tick } from "svelte";
  import { chat, type UiMessage } from "../lib/chat.svelte";

  // Scrollback, windowed.
  //
  // Two separate bounds, and conflating them is the usual bug:
  //
  //  - **Loading** is paged. `loadOlder()` fetches 50 more from the mirror,
  //    and reaches for the network only when the mirror runs dry.
  //  - **Rendering** is windowed. Only the newest `renderCount` of the loaded
  //    messages are in the DOM; the rest are a spacer whose height is the
  //    measured average row height times the number hidden. Scrolling to the
  //    bottom shrinks the window back, so sitting in a busy channel does not
  //    accumulate thousands of nodes.
  //
  // The spacer height is an estimate, so the scrollbar is approximate while
  // you are scrolled up. That is the trade for not measuring every row, and
  // it is invisible in the case that matters — reading at the bottom.

  let viewport = $state<HTMLDivElement | null>(null);
  let rows = $state<HTMLDivElement | null>(null);
  let averageRow = $state(52);
  let stickToBottom = $state(true);

  const TOP_TRIGGER = 200;
  const BOTTOM_SLACK = 80;

  function atBottom(el: HTMLElement) {
    return el.scrollHeight - el.scrollTop - el.clientHeight < BOTTOM_SLACK;
  }

  async function onScroll() {
    const el = viewport;
    if (!el) return;
    stickToBottom = atBottom(el);
    if (stickToBottom) chat.trimRenderWindow();

    if (el.scrollTop > TOP_TRIGGER) return;

    // Prepending changes scrollHeight, which would yank the view. Measure
    // before, restore after: the content the user was reading stays put.
    const before = el.scrollHeight;
    const grew = chat.growRenderWindow() || (await chat.loadOlder());
    if (!grew) return;
    await tick();
    el.scrollTop += el.scrollHeight - before;
  }

  function measure() {
    if (!rows) return;
    const count = rows.children.length;
    if (count > 0) averageRow = Math.max(24, rows.scrollHeight / count);
  }

  // Follow new messages, but only if the reader was already at the bottom —
  // scrolling someone away from the message they are reading is rude.
  $effect(() => {
    void chat.messages.length;
    if (!viewport || !stickToBottom) return;
    tick().then(() => {
      measure();
      if (viewport) viewport.scrollTop = viewport.scrollHeight;
    });
  });

  // A new channel starts at the bottom, like every chat app ever.
  $effect(() => {
    void chat.activeChannel;
    stickToBottom = true;
  });

  function sameAuthorAsPrevious(list: UiMessage[], index: number) {
    if (index === 0) return false;
    return list[index - 1].author_name === list[index].author_name;
  }

  /** Whether this message offers the author's own edit and delete buttons. */
  function isMine(message: UiMessage) {
    return (
      !message.pending && !message.deleted_at && message.author_id === chat.myId
    );
  }

  async function confirmDelete(message: UiMessage) {
    // Not a dialog: a delete here is the author withdrawing their own words,
    // it is one click to say it again, and a modal for it would be in the way
    // far more often than it would save anybody.
    await chat.deleteMessage(message);
  }
</script>

<div
  bind:this={viewport}
  onscroll={onScroll}
  class="flex-1 overflow-y-auto overscroll-contain px-4 py-3"
>
  {#if chat.hiddenAbove > 0}
    <!-- The messages above the render window, as height rather than nodes. -->
    <div style="height: {chat.hiddenAbove * averageRow}px" aria-hidden="true"></div>
  {:else if chat.loadingOlder}
    <p class="py-2 text-center text-xs text-neutral-600">loading history…</p>
  {:else if !chat.hasMoreHistory && chat.messages.length > 0}
    <p class="py-2 text-center text-xs text-neutral-700">
      the beginning of #{chat.activeChannelName}
    </p>
  {/if}

  <div bind:this={rows}>
    {#each chat.rendered as message, i (message.id)}
      <div
        class="group relative rounded px-1 py-0.5 transition hover:bg-white/[0.02]
          {sameAuthorAsPrevious(chat.rendered, i) ? '' : 'mt-3'}
          {chat.editing?.id === message.id
          ? 'bg-[var(--color-accent)]/5 ring-1 ring-inset ring-[var(--color-accent)]/30'
          : ''}"
      >
        {#if !sameAuthorAsPrevious(chat.rendered, i)}
          <div class="text-xs font-semibold text-[var(--color-accent)]">
            {message.author_name}
          </div>
        {/if}

        {#if message.deleted_at}
          <!-- A tombstone rather than a gap. The words are gone from the
               database and from this disk; what is left is the fact that
               somebody said something and took it back, which is the honest
               thing to show (PROTOCOL.md §5). -->
          <div class="text-sm italic leading-relaxed text-neutral-600">
            message deleted
          </div>
        {:else}
          <div
            class="whitespace-pre-wrap break-words text-sm leading-relaxed
              {message.failed
              ? 'text-red-400'
              : message.pending
                ? 'text-neutral-500'
                : 'text-neutral-200'}"
          >
            {message.content}{#if message.edited_at}<span
                class="ml-1.5 text-[10px] text-neutral-600"
                title="edited">(edited)</span
              >{/if}{#if message.pending}<span
                class="ml-2 text-[10px] uppercase tracking-wide text-neutral-600"
                >sending</span
              >{/if}{#if message.failed}<span
                class="ml-2 text-[10px] uppercase tracking-wide text-red-500"
                >not sent</span
              >{/if}
          </div>
        {/if}

        {#if isMine(message)}
          <!-- Only on your own messages, because only their author may change
               them. Moderating somebody else's is the owner's tool, in M6. -->
          <div
            class="absolute right-1 top-0 hidden gap-1 rounded border border-[var(--color-edge)] bg-[var(--color-panel)] px-1 py-0.5 group-hover:flex group-focus-within:flex"
          >
            <button
              onclick={() => chat.beginEdit(message)}
              disabled={chat.activeStatus === "offline"}
              class="rounded px-1.5 py-0.5 text-[11px] text-neutral-400 transition hover:text-[var(--color-accent)] disabled:opacity-40 disabled:hover:text-neutral-400"
              title={chat.activeStatus === "offline"
                ? "offline — edits need a connection"
                : "Edit"}
            >
              edit
            </button>
            <button
              onclick={() => confirmDelete(message)}
              disabled={chat.activeStatus === "offline"}
              class="rounded px-1.5 py-0.5 text-[11px] text-neutral-400 transition hover:text-red-400 disabled:opacity-40 disabled:hover:text-neutral-400"
              title={chat.activeStatus === "offline"
                ? "offline — deletions need a connection"
                : "Delete"}
            >
              delete
            </button>
          </div>
        {/if}
      </div>
    {/each}
  </div>

  {#if chat.messages.length === 0}
    <p class="grid h-full place-items-center text-sm text-neutral-600">
      {chat.activeChannel ? "nothing here yet — say something" : "no channel selected"}
    </p>
  {/if}
</div>
