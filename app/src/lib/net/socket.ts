// The WebSocket that the connection uses: the part of the WebSocket of React Native that
// it needs, so tests give it a fake socket (src/lib/net/testing.ts) and the app the real one
// (src/lib/native/socket.ts).
//
// efrd sends each frame as one text message. It never sends binary messages, and it
// treats one from the phone as a protocol error.

/** What a socket tells its owner. A socket calls no method after `closed`. */
export interface SocketListener {
  opened(): void;
  /** One message: `data` is a string for a text message. */
  message(data: unknown): void;
  /** The socket closed, or failed (`code` is then 1006). */
  closed(code: number, reason: string): void;
}

/** One WebSocket. */
export interface Socket {
  /** Sends one text message. It can throw when the socket is not open. */
  send(text: string): void;
  close(code?: number, reason?: string): void;
}

/** Opens a socket to `url`, such as `ws://100.64.0.1:7419/`, that reports to `listener`. */
export type SocketFactory = (url: string, listener: SocketListener) => Socket;

/** The close code that the app sends when it closes a socket on purpose. */
export const CLOSE_NORMAL = 1000;

/** The close code of a socket that failed without a close frame. */
export const CLOSE_ABNORMAL = 1006;

/** The close code that the app sends when efrd went silent (the liveness rule). */
export const CLOSE_SILENT = 4000;
