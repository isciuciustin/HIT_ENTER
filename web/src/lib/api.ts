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

  /** First join, or enrolling this device on an existing account. */
  joinServer: (args: {
    endpointId: string;
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
  }) => invoke<string>("create_invite", args),

  pendingMessages: (endpointId: string) =>
    invoke<PendingMessage[]>("pending_messages", { endpointId }),
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
