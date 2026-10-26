// The real WebSocket of React Native for the connection (src/lib/net/socket.ts).
//
// React Native on iOS (SocketRocket) sets the Origin header to the URL's own origin,
// `http://<host>:<port>`, which efrd accepts for its tailnet IPv4 (R3.11 of the
// research). The app sets no headers of its own: no cookie and no credential goes in
// the handshake. React Native fires `error` and then `close`; the listener gets one
// `closed` call for both.

import { CLOSE_ABNORMAL, type SocketFactory } from "@/lib/net/socket";

/** Opens a React Native WebSocket. */
export const nativeSockets: SocketFactory = (url, listener) => {
  const socket = new WebSocket(url);
  let done = false;
  const closed = (code: number, reason: string): void => {
    if (!done) {
      done = true;
      listener.closed(code, reason);
    }
  };
  socket.addEventListener("open", () => {
    if (!done) {
      listener.opened();
    }
  });
  socket.addEventListener("message", (event) => {
    if (!done) {
      listener.message(event.data);
    }
  });
  socket.addEventListener("error", (event) => closed(CLOSE_ABNORMAL, event.message));
  socket.addEventListener("close", (event) =>
    closed(event.code ?? CLOSE_ABNORMAL, event.reason ?? ""),
  );
  return {
    send: (text) => socket.send(text),
    close: (code, reason) => {
      done = true;
      socket.close(code, reason);
    },
  };
};
