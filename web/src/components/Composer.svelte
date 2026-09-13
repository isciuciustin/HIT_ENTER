<script lang="ts">
  import { chat } from "../lib/chat.svelte";

  // The cap comes from `he_proto::limits` over the bridge, so the composer and
  // the server can never disagree about what is too long.
  const MESSAGE_MAX_CHARS = $derived(chat.limits.message_max_chars);

  let draft = $state("");
  let box = $state<HTMLTextAreaElement | null>(null);

  const editing = $derived(chat.editing);
  const tooLong = $derived(draft.length > MESSAGE_MAX_CHARS);
  const canSend = $derived(draft.trim().length > 0 && !tooLong);

  // Clicking "edit" on a message loads it here rather than opening a second
  // box: there is one place text is written in this app, and a message being
  // rewritten is still a message being written.
  $effect(() => {
    const target = chat.editing;
    if (!target) return;
    draft = target.content;
    queueMicrotask(() => {
      resize();
      box?.focus();
      box?.setSelectionRange(draft.length, draft.length);
    });
  });

  async function submit() {
    if (editing) {
      const content = draft;
      draft = "";
      resize();
      await chat.commitEdit(content);
      return;
    }
    if (!canSend) return;
    const content = draft;
    // Cleared first: the bubble is already on screen by the time the send
    // returns, and leaving the text in the box would make it look unsent.
    draft = "";
    resize();
    await chat.send(content);
  }

  function abandonEdit() {
    chat.cancelEdit();
    draft = "";
    resize();
    box?.focus();
  }

  function onKeydown(event: KeyboardEvent) {
    // Escape abandons a rewrite. It must not also clear a fresh draft — that
    // would make one key mean "never mind" in one mode and "lose what you
    // typed" in the other.
    if (event.key === "Escape" && editing) {
      event.preventDefault();
      abandonEdit();
      return;
    }
    // Enter sends. Shift+Enter is a newline. The app is called HIT_ENTER.
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void submit();
    }
  }

  function onInput() {
    resize();
    // Fire and forget, and throttled inside the store: a frame per keystroke
    // would be a request stream per keystroke.
    void chat.noteTyping();
  }

  function resize() {
    if (!box) return;
    box.style.height = "auto";
    box.style.height = `${Math.min(box.scrollHeight, 200)}px`;
  }

  /** "alice is typing", "alice and bob are typing", "several people…". */
  function typingLine(names: string[]): string {
    if (names.length === 1) return `${names[0]} is typing…`;
    if (names.length === 2) return `${names[0]} and ${names[1]} are typing…`;
    return "several people are typing…";
  }
</script>

<div class="border-t border-[var(--color-edge)] px-4 py-3">
  {#if editing}
    <div
      class="mb-1.5 flex items-center gap-2 px-1 text-[11px] text-[var(--color-accent)]"
    >
      <span class="font-semibold uppercase tracking-wide">editing</span>
      <span class="min-w-0 flex-1 truncate text-neutral-500">
        {editing.content}
      </span>
      <button
        onclick={abandonEdit}
        class="shrink-0 rounded px-1.5 py-0.5 text-neutral-400 transition hover:text-neutral-200"
      >
        cancel · esc
      </button>
    </div>
  {/if}

  <div
    class="flex items-end gap-2 rounded-lg border bg-[var(--color-panel)] px-3 py-2
      {tooLong
      ? 'border-red-500/60'
      : editing
        ? 'border-[var(--color-accent)]/50'
        : 'border-[var(--color-edge)]'}"
  >
    <textarea
      bind:this={box}
      bind:value={draft}
      oninput={onInput}
      onkeydown={onKeydown}
      rows="1"
      disabled={!chat.activeChannel}
      placeholder={chat.activeChannel
        ? `message #${chat.activeChannelName}`
        : "no channel selected"}
      class="max-h-48 flex-1 resize-none bg-transparent text-sm text-neutral-100 placeholder:text-neutral-600 focus:outline-none disabled:cursor-not-allowed"
    ></textarea>
    <button
      onclick={submit}
      disabled={!canSend && !editing}
      class="rounded px-2 py-1 text-xs font-semibold text-[var(--color-accent)] transition hover:bg-white/5 disabled:text-neutral-700 disabled:hover:bg-transparent"
    >
      {editing ? "save" : "send"}
    </button>
  </div>

  <div class="mt-1 flex justify-between gap-3 px-1 text-[11px] text-neutral-600">
    <span class="min-w-0 truncate">
      {#if chat.typingNames.length > 0}
        <span class="text-neutral-400">{typingLine(chat.typingNames)}</span>
      {:else if chat.activeStatus === "offline"}
        offline — messages are saved and sent when you reconnect
      {:else if editing}
        enter to save · esc to cancel
      {:else}
        enter to send · shift+enter for a newline
      {/if}
    </span>
    {#if draft.length > MESSAGE_MAX_CHARS - 500}
      <span class={tooLong ? "text-red-400" : ""}>
        {draft.length}/{MESSAGE_MAX_CHARS}
      </span>
    {/if}
  </div>
</div>
