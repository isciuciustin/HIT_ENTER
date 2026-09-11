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
        class="group px-1 py-0.5 {sameAuthorAsPrevious(chat.rendered, i)
          ? ''
          : 'mt-3'}"
      >
        {#if !sameAuthorAsPrevious(chat.rendered, i)}
          <div class="text-xs font-semibold text-[var(--color-accent)]">
            {message.author_name}
          </div>
        {/if}
        <div
          class="whitespace-pre-wrap break-words text-sm leading-relaxed
            {message.failed
            ? 'text-red-400'
            : message.pending
              ? 'text-neutral-500'
              : 'text-neutral-200'}"
        >
          {message.content}{#if message.pending}<span
              class="ml-2 text-[10px] uppercase tracking-wide text-neutral-600"
              >sending</span
            >{/if}{#if message.failed}<span
              class="ml-2 text-[10px] uppercase tracking-wide text-red-500"
              >not sent</span
            >{/if}
        </div>
      </div>
    {/each}
  </div>

  {#if chat.messages.length === 0}
    <p class="grid h-full place-items-center text-sm text-neutral-600">
      {chat.activeChannel ? "nothing here yet — say something" : "no channel selected"}
    </p>
  {/if}
</div>
