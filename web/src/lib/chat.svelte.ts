// The app's state, in one place.
//
// Svelte 5 runes: `$state` on class fields, and nothing reactive hiding in a
// `let`. Components read this and call its methods; none of them talk to the
// bridge directly.
//
// The one idea worth reading before the code: **a message on screen is either
// mirrored or pending, never both.** A pending bubble is keyed by its nonce
// and has no server id yet. When the server echoes that nonce back, the
// pending bubble is replaced in place by the authoritative row. That swap is
// the whole reason hitting enter feels instant (PLAN §9).

import {
  api,
  asError,
  onConnection,
  onMessage,
  type Channel,
  type CommandError,
  type ConnectionStatus,
  type Limits,
  type Message,
  type ServerSummary,
} from "./api";

/** A message as the pane renders it. */
export type UiMessage = Message & {
  /** Set while we are waiting for the server to echo this back. */
  nonce?: string;
  pending?: boolean;
  failed?: boolean;
};

/** How many messages to fetch per page, and to render at a time. */
const PAGE = 50;
const RENDER_WINDOW = 120;

function pendingId(nonce: string) {
  return `pending:${nonce}`;
}

class Chat {
  servers = $state<ServerSummary[]>([]);
  activeServer = $state<string | null>(null);
  channels = $state<Channel[]>([]);
  activeChannel = $state<string | null>(null);
  /** Oldest first — the order the pane reads in. */
  messages = $state<UiMessage[]>([]);
  status = $state<Record<string, ConnectionStatus>>({});
  deviceId = $state<string>("");
  version = $state<string>("");
  /** Filled from `app_info`; never hard-coded here. */
  limits = $state<Limits>({
    message_max_chars: 4000,
    username_min_chars: 3,
    username_max_chars: 32,
    password_min_bytes: 8,
    channel_name_max_chars: 64,
  });
  error = $state<CommandError | null>(null);
  /** True while an older page is in flight, so scrolling cannot ask twice. */
  loadingOlder = $state(false);
  /** False once a page comes back short: there is no more history. */
  hasMoreHistory = $state(true);
  /** How many of `messages` to actually put in the DOM. */
  renderCount = $state(RENDER_WINDOW);

  get activeServerSummary(): ServerSummary | null {
    return (
      this.servers.find((s) => s.endpoint_id === this.activeServer) ?? null
    );
  }

  get activeStatus(): ConnectionStatus {
    if (!this.activeServer) return "offline";
    return this.status[this.activeServer] ?? "offline";
  }

  get activeChannelName(): string {
    return (
      this.channels.find((c) => c.id === this.activeChannel)?.name ?? ""
    );
  }

  /** The slice that is actually rendered. See `MessagePane` for why. */
  get rendered(): UiMessage[] {
    const from = Math.max(0, this.messages.length - this.renderCount);
    return this.messages.slice(from);
  }

  get hiddenAbove(): number {
    return Math.max(0, this.messages.length - this.renderCount);
  }

  async init() {
    const info = await api.appInfo();
    this.deviceId = info.device_id;
    this.version = info.version;
    this.limits = info.limits;

    await onMessage((e) => this.onIncoming(e.server, e.message, e.nonce));
    await onConnection((e) => {
      this.status = { ...this.status, [e.server]: e.status };
      const server = this.servers.find((s) => s.endpoint_id === e.server);
      if (server) server.connected = e.status !== "offline";
    });

    await this.refreshServers();

    // Reconnect to everything we already belong to. The device key is the
    // login, so this costs no password and no prompt (PLAN §3).
    for (const server of this.servers) {
      void this.connect(server.endpoint_id);
    }
    if (this.servers.length > 0) {
      await this.selectServer(this.servers[0].endpoint_id);
    }
  }

  async refreshServers() {
    this.servers = await api.listServers();
    for (const server of this.servers) {
      this.status[server.endpoint_id] = server.status;
    }
  }

  async connect(endpointId: string) {
    try {
      await api.connectServer(endpointId);
      await this.refreshServers();
      // Whatever we missed while the app was closed.
      if (this.activeServer === endpointId && this.activeChannel) {
        await this.syncActiveChannel();
      }
    } catch (e) {
      // Offline is an ordinary state here, not a failure worth a banner: the
      // mirror already painted the app.
      const err = asError(e);
      console.warn(`could not connect to ${endpointId}: ${err.message}`);
      this.status = { ...this.status, [endpointId]: "offline" };
    }
  }

  async join(args: {
    endpointId: string;
    username: string;
    password: string;
    invite?: string;
  }) {
    this.error = null;
    try {
      const summary = await api.joinServer(args);
      await this.refreshServers();
      await this.selectServer(summary.endpoint_id);
      return true;
    } catch (e) {
      this.error = asError(e);
      return false;
    }
  }

  async forget(endpointId: string) {
    await api.forgetServer(endpointId);
    await this.refreshServers();
    if (this.activeServer === endpointId) {
      this.activeServer = null;
      this.channels = [];
      this.activeChannel = null;
      this.messages = [];
      if (this.servers.length > 0) {
        await this.selectServer(this.servers[0].endpoint_id);
      }
    }
  }

  async selectServer(endpointId: string) {
    this.activeServer = endpointId;
    // Cached: this is instant, and correct with the network off.
    this.channels = await api.channels(endpointId);
    const first = this.channels[0]?.id ?? null;
    await this.selectChannel(first);
  }

  async selectChannel(channelId: string | null) {
    this.activeChannel = channelId;
    this.messages = [];
    this.renderCount = RENDER_WINDOW;
    this.hasMoreHistory = true;
    if (!this.activeServer || !channelId) return;

    // Paint from disk first…
    const cached = await api.history({
      endpointId: this.activeServer,
      channelId,
      limit: PAGE,
    });
    this.messages = cached.slice().reverse();
    await this.restorePending(channelId);

    // …then ask the network for anything newer.
    await this.syncActiveChannel();
  }

  /** Pulls the newest page from the server and merges it in. */
  async syncActiveChannel() {
    if (!this.activeServer || !this.activeChannel) return;
    const server = this.activeServer;
    const channel = this.activeChannel;
    try {
      const fresh = await api.syncChannel({
        endpointId: server,
        channelId: channel,
        limit: PAGE,
      });
      // The user may have switched channels while this was in flight.
      if (this.activeServer !== server || this.activeChannel !== channel) return;
      for (const message of fresh.slice().reverse()) {
        this.merge(message);
      }
    } catch (e) {
      console.warn(`sync failed: ${asError(e).message}`);
    }
  }

  /** Loads the page before the oldest message on screen. */
  async loadOlder(): Promise<boolean> {
    if (this.loadingOlder || !this.hasMoreHistory) return false;
    if (!this.activeServer || !this.activeChannel) return false;
    const oldest = this.messages.find((m) => !m.pending);
    if (!oldest) return false;

    this.loadingOlder = true;
    const server = this.activeServer;
    const channel = this.activeChannel;
    try {
      let page = await api.history({
        endpointId: server,
        channelId: channel,
        before: oldest.id,
        limit: PAGE,
      });
      // The mirror ran out. The server may still have more — that is exactly
      // what backfill is for, and whatever comes back is written to disk on
      // the way through, so next time the mirror answers.
      if (page.length < PAGE) {
        const older = await api.syncChannel({
          endpointId: server,
          channelId: channel,
          before: oldest.id,
          limit: PAGE,
        });
        if (older.length > 0) page = older;
      }
      if (this.activeServer !== server || this.activeChannel !== channel) {
        return false;
      }
      if (page.length === 0) {
        this.hasMoreHistory = false;
        return false;
      }
      // Newest first from both sources; prepend oldest first.
      const known = new Set(this.messages.map((m) => m.id));
      const older = page
        .slice()
        .reverse()
        .filter((m) => !known.has(m.id));
      if (older.length === 0) {
        this.hasMoreHistory = false;
        return false;
      }
      this.messages = [...older, ...this.messages];
      this.renderCount += older.length;
      return true;
    } finally {
      this.loadingOlder = false;
    }
  }

  /** Renders a bubble immediately, then sends. */
  async send(content: string) {
    const trimmed = content.trim();
    if (!trimmed || !this.activeServer || !this.activeChannel) return;

    const nonce = crypto.randomUUID();
    const me = this.activeServerSummary?.username ?? "you";

    this.messages = [
      ...this.messages,
      {
        id: pendingId(nonce),
        channel_id: this.activeChannel,
        author_id: "",
        author_name: me,
        content: trimmed,
        nonce,
        pending: true,
      },
    ];

    try {
      const sent = await api.sendMessage({
        endpointId: this.activeServer,
        channelId: this.activeChannel,
        content: trimmed,
        nonce,
      });
      // Not delivered means there was no connection. The message is in the
      // outbox on disk; the bubble stays pending rather than lying about it.
      if (!sent.delivered) this.markPending(nonce, false);
    } catch (e) {
      this.markPending(nonce, true);
      this.error = asError(e);
    }
  }

  /** Restores outbox entries as pending bubbles after a restart. */
  private async restorePending(channelId: string) {
    if (!this.activeServer) return;
    const me = this.activeServerSummary?.username ?? "you";
    const pending = await api.pendingMessages(this.activeServer);
    const extra: UiMessage[] = pending
      .filter((p) => p.channel_id === channelId)
      .map((p) => ({
        id: pendingId(p.nonce),
        channel_id: p.channel_id,
        author_id: "",
        author_name: me,
        content: p.content,
        nonce: p.nonce,
        pending: true,
      }));
    if (extra.length > 0) this.messages = [...this.messages, ...extra];
  }

  private markPending(nonce: string, failed: boolean) {
    this.messages = this.messages.map((m) =>
      m.nonce === nonce ? { ...m, pending: !failed, failed } : m,
    );
  }

  private onIncoming(server: string, message: Message, nonce?: string) {
    if (server !== this.activeServer) return;
    if (message.channel_id !== this.activeChannel) return;
    this.merge(message, nonce);
  }

  /**
   * Puts a message in the list exactly once.
   *
   * Three cases, in order: it replaces one of our own pending bubbles (the
   * nonce says so); we already have it (a backfill page overlapping a live
   * event, which happens on every reconnect); or it is new.
   */
  private merge(message: Message, nonce?: string) {
    if (nonce) {
      const index = this.messages.findIndex((m) => m.nonce === nonce);
      if (index >= 0) {
        const next = this.messages.slice();
        next[index] = { ...message };
        this.messages = next;
        return;
      }
    }
    if (this.messages.some((m) => m.id === message.id)) return;

    // Ids are UUIDv7, so "newer than everything" is the common case and a
    // straight append is right. Anything else is inserted by id order.
    const last = this.messages[this.messages.length - 1];
    if (!last || last.id < message.id) {
      this.messages = [...this.messages, message];
      return;
    }
    const at = this.messages.findIndex((m) => m.id > message.id);
    const next = this.messages.slice();
    next.splice(at < 0 ? next.length : at, 0, message);
    this.messages = next;
  }

  /** Called when the pane is scrolled back to the bottom. */
  trimRenderWindow() {
    if (this.renderCount > RENDER_WINDOW) this.renderCount = RENDER_WINDOW;
  }

  growRenderWindow(): boolean {
    if (this.hiddenAbove === 0) return false;
    this.renderCount += RENDER_WINDOW;
    return true;
  }
}

export const chat = new Chat();
