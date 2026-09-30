import { keepalive } from "./global-setup";

export default async function globalTeardown(): Promise<void> {
  const state = keepalive();
  if (!state) return;
  state.closing = true;
  clearTimeout(state.retry);
  state.ws?.close();
}
