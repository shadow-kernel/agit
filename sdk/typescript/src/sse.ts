// Minimal Server-Sent Events parser over a `ReadableStream<Uint8Array>` (what `fetch` gives
// us). Used instead of `EventSource` so the stream can carry `Authorization` and
// `X-Agit-*` headers and works in Node 18+ without any polyfill.

export interface SseMessage {
  /** Value of the last `id:` field seen (sticky across messages, per the spec). */
  id?: string;
  /** Value of the `event:` field, if any. */
  event?: string;
  /** Joined `data:` lines (separated by `\n`). */
  data: string;
  /** Value of a `retry:` field attached to this message, in milliseconds. */
  retry?: number;
}

/**
 * Parse an SSE byte stream into messages. Handles `\n`, `\r\n` and `\r` line endings,
 * multi-line `data:`, comments (`: keepalive`), `id:`, `event:` and `retry:` fields.
 * Cancels the underlying reader when the consumer stops early.
 */
export async function* parseSseStream(stream: ReadableStream<Uint8Array>): AsyncGenerator<SseMessage, void, undefined> {
  const reader = stream.getReader();
  const decoder = new TextDecoder("utf-8");
  let buffer = "";
  let lastId: string | undefined;
  let data: string[] = [];
  let event: string | undefined;
  let retry: number | undefined;
  let sawData = false;

  const flush = (): SseMessage | undefined => {
    if (!sawData) {
      event = undefined;
      retry = undefined;
      return undefined;
    }
    const msg: SseMessage = { data: data.join("\n") };
    if (lastId !== undefined) msg.id = lastId;
    if (event !== undefined) msg.event = event;
    if (retry !== undefined) msg.retry = retry;
    data = [];
    event = undefined;
    retry = undefined;
    sawData = false;
    return msg;
  };

  const handleLine = (line: string): SseMessage | undefined => {
    if (line === "") return flush();
    if (line.startsWith(":")) return undefined;
    const colon = line.indexOf(":");
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? "" : line.slice(colon + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    switch (field) {
      case "data":
        data.push(value);
        sawData = true;
        break;
      case "event":
        event = value;
        break;
      case "id":
        if (!value.includes("\0")) lastId = value;
        break;
      case "retry": {
        const n = Number(value);
        if (Number.isInteger(n) && n >= 0) retry = n;
        break;
      }
      default:
        break;
    }
    return undefined;
  };

  try {
    for (;;) {
      const { value, done } = await reader.read();
      buffer += done ? decoder.decode() : decoder.decode(value, { stream: true });
      // Normalise line endings, but never split a trailing "\r" that may be followed by "\n" in the next chunk.
      let searchable = buffer;
      let carry = "";
      if (!done && searchable.endsWith("\r")) {
        carry = "\r";
        searchable = searchable.slice(0, -1);
      }
      const lines = searchable.split(/\r\n|\r|\n/);
      const tail = done ? "" : (lines.pop() ?? "");
      for (const line of lines) {
        const msg = handleLine(line);
        if (msg) yield msg;
      }
      buffer = tail + carry;
      if (done) {
        if (buffer !== "") {
          const msg = handleLine(buffer);
          if (msg) yield msg;
        }
        const last = flush();
        if (last) yield last;
        return;
      }
    }
  } finally {
    try {
      await reader.cancel();
    } catch {
      /* already closed */
    }
    try {
      reader.releaseLock();
    } catch {
      /* ignore */
    }
  }
}
