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
  onDeleted,
  onEdited,
  onHost,
  onLink,
  onMessage,
  onPresence,
  onPresenceSync,
  onTyping,
  TYPING_THROTTLE_MS,
  TYPING_TIMEOUT_MS,
  type Channel,
  type CommandError,
  type ConnectionStatus,
  type HostStatus,
  type Limits,
  type Member,
  type Message,
  type ServerSummary,
  type SettingsView,
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

/** Whether this window is the one the user is actually looking at. */
function isFocused(): boolean {
  return typeof document === "undefined" || document.hasFocus();
}

/** Somebody composing, and when we last heard so. */
type Typist = { username: string; at: number };

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
  /** The space this machine serves, or could serve (PLAN §2.1). */
  host = $state<HostStatus | null>(null);
  settings = $state<SettingsView | null>(null);
  /**
   * A link the desktop handed us, waiting for the user to look at it.
   *
   * Parked here rather than acted on: clicking a URL must not be enough to
   * enrol this device in a stranger's space.
   */
  pendingLink = $state<string | null>(null);
  /** True while an older page is in flight, so scrolling cannot ask twice. */
  loadingOlder = $state(false);
  /** False once a page comes back short: there is no more history. */
  hasMoreHistory = $state(true);
  /** How many of `messages` to actually put in the DOM. */
  renderCount = $state(RENDER_WINDOW);
  /** The active space's roster, from the mirror — so it renders offline. */
  members = $state<Member[]>([]);
  /**
   * Who has a live session right now, by user id.
   *
   * Never read from disk and never written to it. A stored "online" would be
   * a lie the moment the app closed (PLAN §6).
   */
  online = $state<Set<string>>(new Set());
  /** Unread counts per channel id, for the badge in the rail. */
  unread = $state<Record<string, number>>({});
  /** Who is composing in the active channel, by user id. */
  typists = $state<Record<string, Typist>>({});
  /** The message being rewritten in the composer, if any. */
  editing = $state<UiMessage | null>(null);

  /** When we last told the server we were typing. Throttled to match it. */
  #lastTypingSent = 0;
  /** Sweeps expired typing indicators; there is no "stopped typing" event. */
  #typingTimer: ReturnType<typeof setInterval> | null = null;

  /** True when the active space is the one this machine is serving. */
  get activeIsOwnSpace(): boolean {
    return (
      this.host?.running === true &&
      this.host.endpoint_id === this.activeServer
    );
  }

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

  /** This account's own id on the active space, for "is this mine?". */
  get myId(): string | null {
    return this.activeServerSummary?.user_id ?? null;
  }

  /** The roster, online first, so the people who can answer are at the top. */
  get roster(): (Member & { online: boolean })[] {
    return this.members
      .map((m) => ({ ...m, online: this.online.has(m.id) }))
      .sort((a, b) => {
        if (a.online !== b.online) return a.online ? -1 : 1;
        if (a.is_owner !== b.is_owner) return a.is_owner ? -1 : 1;
        return a.username.localeCompare(b.username);
      });
  }

  get onlineCount(): number {
    return this.members.filter((m) => this.online.has(m.id)).length;
  }

  /** The names to put under the composer, never more than a sentence of them. */
  get typingNames(): string[] {
    return Object.values(this.typists).map((t) => t.username);
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
    await onEdited((e) => this.onIncoming(e.server, e.message));
    await onDeleted((e) => this.onDeleted(e.server, e.id, e.deleted_at));
    await onPresence((e) => this.onPresence(e.server, e.user_id, e.online));
    await onPresenceSync((e) => {
      if (e.server === this.activeServer) this.online = new Set(e.online);
    });
    await onTyping((e) => this.onTyping(e.server, e.channel_id, e.user_id, e.username));
    await onConnection((e) => {
      this.status = { ...this.status, [e.server]: e.status };
      const server = this.servers.find((s) => s.endpoint_id === e.server);
      if (server) server.connected = e.status !== "offline";
      // A reconnect just resumed, so the counts on disk have moved. A
      // disconnect empties the room: nobody is online through a dead session.
      if (e.server === this.activeServer) {
        void this.refreshUnread();
        if (e.status === "offline") this.online = new Set();
      }
    });
    await onHost(() => void this.refreshHost());
    await onLink((link) => (this.pendingLink = link));

    // There is no "stopped typing" event to lose, so indicators expire on
    // their own. Sweeping is cheaper and more robust than a timer per typist.
    this.#typingTimer = setInterval(() => this.expireTypists(), 1_000);

    await Promise.all([this.refreshHost(), this.refreshSettings()]);
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

  async refreshHost() {
    this.host = await api.hostStatus();
  }

  async refreshSettings() {
    this.settings = await api.networkSettings();
  }

  /**
   * Creates a space on this machine and joins it.
   *
   * The join goes over iroh like anyone else's would — there is no local
   * shortcut, which is why the owner's own client exercises the handshake,
   * the enrolment and the mirror on every run (PLAN §2.1).
   */
  async createSpace(args: { name: string; username: string; password: string }) {
    this.error = null;
    try {
      const created = await api.createSpace(args);
      this.host = created.host;
      await this.refreshServers();
      await this.selectServer(created.server.endpoint_id);
      return true;
    } catch (e) {
      this.error = asError(e);
      return false;
    }
  }

  async setHosting(running: boolean) {
    this.error = null;
    try {
      this.host = running ? await api.startHosting() : await api.stopHosting();
      return true;
    } catch (e) {
      this.error = asError(e);
      return false;
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
    address: string;
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
    // Presence belongs to a session, and switching spaces is not one: the
    // people online *here* are a different set, and carrying the old one over
    // would show a green dot next to somebody in another space entirely.
    this.online = new Set();
    this.typists = {};
    // Channels and members are cached, so both are instant and correct with
    // the network off. Presence is not cached and never will be — it comes
    // from the live session, and is empty when there is not one.
    const [channels, members, online] = await Promise.all([
      api.channels(endpointId),
      api.members(endpointId),
      api.online(endpointId),
    ]);
    this.channels = channels;
    this.members = members;
    this.online = new Set(online);
    await this.refreshUnread();
    const first = this.channels[0]?.id ?? null;
    await this.selectChannel(first);
  }

  async refreshUnread() {
    if (!this.activeServer) return;
    const counts = await api.unread(this.activeServer);
    const next: Record<string, number> = {};
    for (const row of counts) next[row.channel_id] = row.unread;
    this.unread = next;
    // The rail shows a per-space total read from the same rows.
    await this.refreshServers();
  }

  async selectChannel(channelId: string | null) {
    this.activeChannel = channelId;
    this.messages = [];
    this.renderCount = RENDER_WINDOW;
    this.hasMoreHistory = true;
    this.typists = {};
    this.cancelEdit();
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
    // Opening a channel is reading it.
    await this.markRead();
  }

  /**
   * Marks the active channel read up to its newest message.
   *
   * Only moves forward, in the mirror, so scrolling back through history is
   * not the same as un-reading it.
   */
  async markRead() {
    if (!this.activeServer || !this.activeChannel) return;
    const newest = [...this.messages].reverse().find((m) => !m.pending);
    if (!newest) return;
    await api.markRead({
      endpointId: this.activeServer,
      channelId: this.activeChannel,
      messageId: newest.id,
    });
    await this.refreshUnread();
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

  /**
   * Tells the space this account is composing.
   *
   * Throttled here as well as on the server, because the server's throttle
   * protects the *space* and this one protects the connection: a frame per
   * keystroke would be a request stream per keystroke.
   */
  async noteTyping() {
    if (!this.activeServer || !this.activeChannel) return;
    if (this.editing) return; // rewriting is not composing
    const now = Date.now();
    if (now - this.#lastTypingSent < TYPING_THROTTLE_MS) return;
    this.#lastTypingSent = now;
    await api.typing({
      endpointId: this.activeServer,
      channelId: this.activeChannel,
    });
  }

  /** Puts a message of ours back in the composer to be rewritten. */
  beginEdit(message: UiMessage) {
    if (message.pending || message.deleted_at) return;
    if (message.author_id !== this.myId) return;
    this.editing = message;
  }

  cancelEdit() {
    this.editing = null;
  }

  /** Commits the rewrite. The authoritative row arrives as an event. */
  async commitEdit(content: string) {
    const target = this.editing;
    const trimmed = content.trim();
    if (!target || !this.activeServer) return false;
    this.cancelEdit();
    // Unchanged text is a cancel, not a round trip that stamps "edited" on a
    // message nobody edited.
    if (!trimmed || trimmed === target.content) return true;
    try {
      await api.editMessage({
        endpointId: this.activeServer,
        messageId: target.id,
        content: trimmed,
      });
      return true;
    } catch (e) {
      this.error = asError(e);
      return false;
    }
  }

  /** Withdraws one of our own messages, text and all. */
  async deleteMessage(message: UiMessage) {
    if (!this.activeServer || message.author_id !== this.myId) return;
    if (this.editing?.id === message.id) this.cancelEdit();
    try {
      await api.deleteMessage({
        endpointId: this.activeServer,
        messageId: message.id,
      });
    } catch (e) {
      this.error = asError(e);
    }
  }

  /** Renders a bubble immediately, then sends. */
  async send(content: string) {
    const trimmed = content.trim();
    if (!trimmed || !this.activeServer || !this.activeChannel) return;
    // Whatever we were saying, we have stopped.
    this.#lastTypingSent = 0;

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
    if (server !== this.activeServer) {
      // Another space. The mirror already has it, so the rail's dot is just a
      // re-read of disk rather than a count kept in the window.
      void this.refreshServers();
      return;
    }
    // Somebody who just said something is not still typing.
    if (message.author_id) this.clearTypist(message.author_id);

    if (message.channel_id !== this.activeChannel) {
      void this.refreshUnread();
      return;
    }
    this.merge(message, nonce);
    // Looking at a channel is reading it — but only if this window is the one
    // being looked at. Coming back to the app and finding nothing marked new
    // is the bug that makes an unread count useless.
    if (isFocused()) void this.markRead();
    else void this.refreshUnread();
  }

  /** Takes a withdrawn message off the screen, leaving a tombstone. */
  private onDeleted(server: string, id: string, deletedAt: number) {
    if (server !== this.activeServer) return;
    if (this.editing?.id === id) this.cancelEdit();
    const index = this.messages.findIndex((m) => m.id === id);
    if (index < 0) {
      // Not on screen, but it may still have been counted as unread.
      void this.refreshUnread();
      return;
    }
    const next = this.messages.slice();
    next[index] = { ...next[index], content: "", deleted_at: deletedAt };
    this.messages = next;
    void this.refreshUnread();
  }

  private onPresence(server: string, userId: string, online: boolean) {
    if (server !== this.activeServer) return;
    const next = new Set(this.online);
    if (online) {
      next.add(userId);
    } else {
      next.delete(userId);
      this.clearTypist(userId);
    }
    this.online = next;
  }

  private onTyping(
    server: string,
    channelId: string,
    userId: string,
    username: string,
  ) {
    if (server !== this.activeServer || channelId !== this.activeChannel) return;
    this.typists = { ...this.typists, [userId]: { username, at: Date.now() } };
  }

  private clearTypist(userId: string) {
    if (!(userId in this.typists)) return;
    const next = { ...this.typists };
    delete next[userId];
    this.typists = next;
  }

  /**
   * Drops indicators nobody renewed.
   *
   * There is no "stopped typing" frame, on purpose: one would be a frame that
   * can be lost, and losing it leaves somebody typing forever. An indicator
   * that has to be renewed cannot get stuck.
   */
  private expireTypists() {
    const cutoff = Date.now() - TYPING_TIMEOUT_MS;
    const live = Object.entries(this.typists).filter(([, t]) => t.at > cutoff);
    if (live.length === Object.keys(this.typists).length) return;
    this.typists = Object.fromEntries(live);
  }

  /**
   * Puts a message in the list exactly once, with the newest version of it.
   *
   * Three cases, in order: it replaces one of our own pending bubbles (the
   * nonce says so); we already have that id, in which case the arriving copy
   * wins; or it is new.
   *
   * The second case is why this is a *replace* rather than a skip. A backfill
   * page overlapping a live event is the common shape of it and replacing
   * changes nothing — but an `edited` event and a `resume` that carries a
   * deletion are the same shape, and skipping those would leave the old text
   * on screen while the mirror held the new.
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
    const existing = this.messages.findIndex((m) => m.id === message.id);
    if (existing >= 0) {
      const next = this.messages.slice();
      next[existing] = { ...message };
      this.messages = next;
      return;
    }

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
