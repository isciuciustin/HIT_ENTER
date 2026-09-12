<script lang="ts">
  // The shell every dialog sits in.
  //
  // It exists for two behaviours that are easy to leave out and obvious when
  // they are missing:
  //
  //  - **Focus starts inside and stays inside.** Without a trap, Tab walks
  //    straight out of the dialog onto the page behind the backdrop — which
  //    the user cannot see — and the next keystroke goes somewhere invisible.
  //    Pressing space there activates whatever button Tab happened to land on.
  //  - **Escape closes it.** A modal with no keyboard exit is a modal you
  //    reach for the mouse to leave.

  import type { Snippet } from "svelte";

  let {
    onClose,
    children,
    wide = false,
  }: { onClose: () => void; children: Snippet; wide?: boolean } = $props();

  let panel = $state<HTMLDivElement | null>(null);

  const FOCUSABLE =
    'input:not([disabled]), textarea:not([disabled]), button:not([disabled]), select:not([disabled]), [href], [tabindex]:not([tabindex="-1"])';

  function focusable(): HTMLElement[] {
    return panel ? Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE)) : [];
  }

  // The first field, not the first button: a dialog that opens with the
  // cursor already where the typing goes is the whole point of opening it.
  $effect(() => {
    if (!panel) return;
    const first =
      panel.querySelector<HTMLElement>("input, textarea") ?? focusable()[0];
    first?.focus();
  });

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;

    const items = focusable();
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement as HTMLElement | null;

    // Focus can end up outside the panel without a Tab having taken it there:
    // the element it was on gets removed when the dialog's content changes —
    // a minted invite replaces the form with a link — and the browser drops
    // focus to <body>. The next Tab then walks the page *behind* the backdrop,
    // where the user cannot see what is selected. Pulling it back in is what
    // makes the trap hold across a re-render.
    const outside = !active || !panel?.contains(active);
    if (outside) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
      return;
    }

    // Wrap at both ends rather than letting the browser walk out of the panel.
    if (event.shiftKey && active === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="fixed inset-0 z-50 grid place-items-center bg-black/70 p-6">
  <div
    bind:this={panel}
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    class="max-h-full w-full overflow-y-auto rounded-xl border border-[var(--color-edge)] bg-[var(--color-panel)] p-5 {wide
      ? 'max-w-lg'
      : 'max-w-md'}"
  >
    {@render children()}
  </div>
</div>
