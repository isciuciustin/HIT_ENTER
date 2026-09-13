// Keyboard shortcuts.
//
// One rule shaped the list: **do not steal a key somebody is using**.
//
// Every shortcut here is either modified with Alt or Ctrl, or is a key the
// composer has no meaning for. That is what lets them all work while the
// cursor is in the composer — which in a chat app is almost always — without
// any of them going off mid-sentence. Switching channel with a half-written
// message is a thing people do on purpose, so it must not need a click first.
//
// Nothing overrides a browser or OS binding a user would miss, which is why
// channel and space switching live on Alt rather than on Ctrl.

export type Shortcut = {
  /** How it reads in the help sheet. */
  keys: string;
  description: string;
};

/** The list, in the order the help sheet shows it. */
export const SHORTCUTS: { group: string; items: Shortcut[] }[] = [
  {
    group: "getting around",
    items: [
      { keys: "Alt + ↑ / ↓", description: "previous / next channel" },
      { keys: "Alt + Shift + ↑ / ↓", description: "previous / next space" },
      { keys: "Esc", description: "close a dialog, or cancel an edit" },
    ],
  },
  {
    group: "writing",
    items: [
      { keys: "Enter", description: "send" },
      { keys: "Shift + Enter", description: "newline" },
      { keys: "↑ (empty composer)", description: "edit your last message" },
    ],
  },
  {
    group: "the rest",
    items: [
      { keys: "Ctrl + ,", description: "network settings" },
      { keys: "Ctrl + /", description: "this list" },
    ],
  },
];

/**
 * Steps an index by `delta`, stopping at the ends rather than wrapping.
 *
 * Wrapping would make "next channel" jump to the top of the list, which reads
 * as a bug the first time it happens and as a nuisance every time after.
 */
export function step(length: number, current: number, delta: number): number | null {
  if (length === 0) return null;
  const next = (current < 0 ? (delta > 0 ? -1 : length) : current) + delta;
  if (next < 0 || next >= length) return null;
  return next;
}
