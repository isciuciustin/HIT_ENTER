// The Rust bridge, typed.
//
// Every command is here and nowhere else, so a change to a `#[tauri::command]`
// signature breaks in one file rather than in six components.
//
// The split that matters: `history` and `channels` read the local mirror and
// never touch the network, while `syncChannel` is the call that goes out to
// the server. The UI paints from the first pair and tops up with the second,
// which is why it opens instantly and works with the wifi off.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

/**
 * Puts text on the system clipboard.
 *
 * Not `navigator.clipboard.writeText`: WebKitGTK rejects it, and it rejects it
 * *silently*, so the button says "copied" and the clipboard stays empty. An
 * invite link that cannot be copied is a space nobody can join, so this goes
 * through the OS. Returns whether it worked, so a caller can say so honestly.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    await writeText(text);
    return true;
  } catch (e) {
    console.error("could not write to the clipboard", e);
    return false;
  }
}

export type Channel = {
  id: string;
  name: string;
  topic: string | null;
  position: number;
};

export type Message = {
  id: string;
  channel_id: string;
  author_id: string;
  author_name: string;
  content: string;
  edited_at?: number | null;
  deleted_at?: number | null;
};

export type ConnectionStatus = "direct" | "relayed" | "connecting" | "offline";

export type ServerSummary = {
  endpoint_id: string;
  name: string;
  relay_url: string | null;
  username: string;
  added_at: number;
  last_seen: number | null;
  connected: boolean;
  status: ConnectionStatus;
};

/** The shared validation rules, straight from `he_proto::limits`. */
export type Limits = {
  message_max_chars: number;
  username_min_chars: number;
  username_max_chars: number;
  password_min_bytes: number;
  channel_name_max_chars: number;
};

export type AppInfo = {
  version: string;
  protocol: string;
  protocol_version: number;
  device_id: string;
  limits: Limits;
};

/** Which relays to use when a direct path cannot be punched (PLAN §4). */
export type Relays =
  | { mode: "n0" }
  | { mode: "custom"; urls: string[] }
  | { mode: "disabled" };

export type NetworkConfig = {
  relays: Relays;
  n0_discovery: boolean;
  mdns_discovery: boolean;
};

export type SettingsView = {
  network: NetworkConfig;
  /** Built from the same struct in Rust, never reassembled here. */
  summary: string;
  self_contained: boolean;
  data_dir: string;
  settings_path: string;
};

export type SettingsSaved = {
  settings: SettingsView;
  /** The client endpoint was bound at startup and holds every open session. */
  restart_required: boolean;
  host_restarted: boolean;
};

/** The space this machine serves, or could serve. */
export type HostStatus = {
  space_exists: boolean;
  running: boolean;
  auto_start: boolean;
  /** The *space's* key — not this device's. Separate keys, separate lives. */
  endpoint_id?: string;
  space_name: string;
  owner?: string;
  link?: string;
  reachable_remotely: boolean;
  data_dir: string;
};

export type SpaceCreated = { host: HostStatus; server: ServerSummary };

export type Invite = {
  code: string;
  /** The whole invite, and what to paste into a chat message (PLAN §5). */
  link: string;
  reachable_remotely: boolean;
};

export type ParsedLink = {
  endpoint_id: string;
  code?: string;
  relay_url?: string;
  known: boolean;
  is_own_space: boolean;
};

export type Sent = { nonce: string; delivered: boolean };

export type PendingMessage = {
  nonce: string;
  channel_id: string;
  content: string;
  created_at: number;
};

/** What a failed command gives back. `code` is the server's own `ErrorCode`. */
export type CommandError = {
  code?: string;
  message: string;
  retry_after?: number;
};

/** Tauri rejects with the serialized error; this narrows it back to a type. */
export function asError(e: unknown): CommandError {
  if (e && typeof e === "object" && "message" in e) return e as CommandError;
  return { message: String(e) };
}

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),

  listServers: () => invoke<ServerSummary[]>("list_servers"),

  /**
   * First join, or enrolling this device on an existing account.
   *
   * `address` is whatever the user pasted — a `hitenter://` link, a ticket, or
   * a bare EndpointId. A link carries its own invite code; `invite` overrides.
   */
  joinServer: (args: {
    address: string;
    username: string;
    password: string;
    invite?: string;
  }) => invoke<ServerSummary>("join_server", args),

  /** Reconnecting a device that is already enrolled. No password. */
  connectServer: (endpointId: string) =>
    invoke<ServerSummary>("connect_server", { endpointId }),

  disconnectServer: (endpointId: string) =>
    invoke<void>("disconnect_server", { endpointId }),

  forgetServer: (endpointId: string) =>
    invoke<void>("forget_server", { endpointId }),

  /** Cached. Never touches the network. */
  channels: (endpointId: string) =>
    invoke<Channel[]>("channels", { endpointId }),

  /** Cached history, newest first, ending just before `before`. */
  history: (args: {
    endpointId: string;
    channelId: string;
    before?: string;
    limit?: number;
  }) => invoke<Message[]>("history", args),

  /** Goes to the server. Returns [] when offline, which is not an error. */
  syncChannel: (args: {
    endpointId: string;
    channelId: string;
    before?: string;
    limit?: number;
  }) => invoke<Message[]>("sync_channel", args),

  sendMessage: (args: {
    endpointId: string;
    channelId: string;
    content: string;
    nonce: string;
  }) => invoke<Sent>("send_message", args),

  createInvite: (args: {
    endpointId: string;
    expiresIn?: number;
    maxUses?: number;
  }) => invoke<Invite>("create_invite", args),

  pendingMessages: (endpointId: string) =>
    invoke<PendingMessage[]>("pending_messages", { endpointId }),

  /** Pure parsing: nothing is dialled and nothing is stored. */
  parseLink: (text: string) => invoke<ParsedLink>("parse_link", { text }),

  hostStatus: () => invoke<HostStatus>("host_status"),

  /** Creates a space on this machine and joins it over the network. */
  createSpace: (args: { name: string; username: string; password: string }) =>
    invoke<SpaceCreated>("create_space", args),

  startHosting: () => invoke<HostStatus>("start_hosting"),
  stopHosting: () => invoke<HostStatus>("stop_hosting"),

  networkSettings: () => invoke<SettingsView>("network_settings"),
  setNetworkSettings: (network: NetworkConfig) =>
    invoke<SettingsSaved>("set_network_settings", { network }),
};

export type MessageEvent = {
  server: string;
  message: Message;
  /** Present only when this client sent it — that is the whole point. */
  nonce?: string;
};

export type ConnectionEvent = { server: string; status: ConnectionStatus };

export const onMessage = (cb: (e: MessageEvent) => void): Promise<UnlistenFn> =>
  listen<MessageEvent>("he://message", (e) => cb(e.payload));

export const onConnection = (
  cb: (e: ConnectionEvent) => void,
): Promise<UnlistenFn> =>
  listen<ConnectionEvent>("he://connection", (e) => cb(e.payload));

/** Hosting started, stopped, or was rebound. Details come from `hostStatus`. */
export const onHost = (cb: () => void): Promise<UnlistenFn> =>
  listen<null>("he://host", () => cb());

/**
 * A `hitenter://` link arrived from the desktop.
 *
 * It opens the join dialog and nothing else — a link that joined on arrival
 * would make clicking a URL enough to enrol this device somewhere.
 */
export const onLink = (cb: (link: string) => void): Promise<UnlistenFn> =>
  listen<string>("he://link", (e) => cb(e.payload));
