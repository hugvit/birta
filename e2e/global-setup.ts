// birta shuts down once no socket has been open for 5 s, and the specs can leave the
// shared server idle that long, so one socket is held open for the whole run.
const URL = "ws://127.0.0.1:3456/ws";
const RETRY_MS = 200;
const OPEN_TIMEOUT_MS = 10_000;

export interface Keepalive {
  closing: boolean;
  ws?: WebSocket;
  retry?: ReturnType<typeof setTimeout>;
}

const KEY = "__birtaKeepalive";

export function keepalive(): Keepalive | undefined {
  return (globalThis as Record<string, unknown>)[KEY] as Keepalive | undefined;
}

/** Opens the socket; once open, a drop reconnects until teardown sets `closing`. */
function connect(state: Keepalive): Promise<void> {
  return new Promise((resolve, reject) => {
    // Node sends no Origin header, which the server's guard accepts.
    const ws = new WebSocket(URL);
    let opened = false;
    state.ws = ws;
    ws.onopen = () => {
      opened = true;
      // A hello for no doc keeps the connection counted without asking for every render.
      ws.send(JSON.stringify({ type: "hello", path: "", tab: "e2e-keepalive" }));
      resolve();
    };
    ws.onclose = () => {
      if (!opened) {
        reject(new Error(`keepalive socket to ${URL} did not open`));
        return;
      }
      if (state.closing) return;
      state.retry = setTimeout(function again() {
        if (state.closing) return;
        connect(state).catch(() => {
          state.retry = setTimeout(again, RETRY_MS);
        });
      }, RETRY_MS);
    };
  });
}

export default async function globalSetup(): Promise<void> {
  const state: Keepalive = { closing: false };
  (globalThis as Record<string, unknown>)[KEY] = state;
  const deadline = Date.now() + OPEN_TIMEOUT_MS;
  for (;;) {
    try {
      await connect(state);
      return;
    } catch (e) {
      if (Date.now() > deadline) throw e;
      await new Promise((r) => setTimeout(r, RETRY_MS));
    }
  }
}
