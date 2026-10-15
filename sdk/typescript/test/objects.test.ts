import { describe, expect, it } from "vitest";
import { ObjectStreamError, decodeObjectStream, encodeObjectFrame, encodeObjectStream, iterateObjectStream, objectKindFromTag, objectKindToTag, parseSseStream } from "../src/index.js";

const te = new TextEncoder();

describe("object stream codec", () => {
  it("encodes kind:u8 | len:u32be | payload", () => {
    const f = encodeObjectFrame({ kind: "tree", payload: te.encode("abc") });
    expect(Array.from(f)).toEqual([2, 0, 0, 0, 3, 97, 98, 99]);
  });

  it("round-trips multiple frames and keeps order", () => {
    const frames = [
      { kind: "blob" as const, payload: te.encode("hello") },
      { kind: "tree" as const, payload: new Uint8Array(0) },
      { kind: "commit" as const, payload: te.encode('{"tree":"x"}') },
    ];
    const bytes = encodeObjectStream(frames);
    expect(bytes.byteLength).toBe(5 * 3 + 5 + 0 + 12);
    const decoded = decodeObjectStream(bytes);
    expect(decoded.map((d) => d.kind)).toEqual(["blob", "tree", "commit"]);
    expect(decoded.map((d) => new TextDecoder().decode(d.payload))).toEqual(["hello", "", '{"tree":"x"}']);
  });

  it("decodes from a subarray view with a non-zero byteOffset", () => {
    const inner = encodeObjectStream([{ kind: "blob", payload: te.encode("xy") }]);
    const padded = new Uint8Array(inner.byteLength + 3);
    padded.set(inner, 3);
    const frames = decodeObjectStream(padded.subarray(3));
    expect(new TextDecoder().decode(frames[0]?.payload)).toBe("xy");
  });

  it("rejects truncated streams and unknown kinds", () => {
    expect(() => decodeObjectStream(new Uint8Array([1, 0, 0]))).toThrow(ObjectStreamError);
    expect(() => decodeObjectStream(new Uint8Array([1, 0, 0, 0, 9, 1, 2]))).toThrow(/truncated object stream payload/);
    expect(() => Array.from(iterateObjectStream(new Uint8Array([9, 0, 0, 0, 0])))).toThrow(/bad object kind 9/);
    expect(objectKindFromTag(3)).toBe("commit");
    expect(objectKindFromTag(0)).toBeUndefined();
    expect(objectKindToTag("blob")).toBe(1);
  });
});

function streamOf(chunks: string[]): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(controller) {
      for (const c of chunks) controller.enqueue(te.encode(c));
      controller.close();
    },
  });
}

describe("SSE parser", () => {
  it("handles chunk boundaries, CRLF, comments, multi-line data, id and retry", async () => {
    const chunks = [": hello\r\n\r\nid: 1\nevent: commit\nda", "ta: {\"a\":1}\n\nretry: 250\ndata: line1\r\ndata: line2\r", "\n\r\ndata: tail"];
    const out = [];
    for await (const m of parseSseStream(streamOf(chunks))) out.push(m);
    expect(out).toEqual([
      { id: "1", event: "commit", data: '{"a":1}' },
      { id: "1", data: "line1\nline2", retry: 250 },
      { id: "1", data: "tail" },
    ]);
  });

  it("cancels the source when the consumer stops early", async () => {
    let cancelled = false;
    const src = new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(te.encode("data: a\n\ndata: b\n\n"));
      },
      cancel() {
        cancelled = true;
      },
    });
    for await (const m of parseSseStream(src)) {
      expect(m.data).toBe("a");
      break;
    }
    expect(cancelled).toBe(true);
  });
});
