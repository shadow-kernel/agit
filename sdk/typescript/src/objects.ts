// Object stream codec (content type `application/x-agit-objects`):
// repeated frames `kind:u8 | len:u32 big-endian | payload`; kind 1 blob, 2 tree, 3 commit.
//
// Object ids are BLAKE3 digests computed by the server when objects are stored. This SDK
// deliberately ships without a BLAKE3 implementation (zero dependencies), so it cannot
// compute ids locally: creating commits by uploading hand-built objects and then pointing
// refs at them ("sync") is out of scope here. Use the `agit` CLI or the Rust/Python
// protocol clients for that; `uploadObjects`/`fetchObjects` remain available for
// mirroring streams between servers.

import type { ObjectKind } from "./types.js";

export const OBJECT_STREAM_CONTENT_TYPE = "application/x-agit-objects";

export const OBJECT_KIND_TAG: Readonly<Record<ObjectKind, number>> = Object.freeze({ blob: 1, tree: 2, commit: 3 });

const TAG_KIND: Readonly<Record<number, ObjectKind>> = Object.freeze({ 1: "blob", 2: "tree", 3: "commit" });

export interface ObjectFrame {
  kind: ObjectKind;
  payload: Uint8Array;
}

export class ObjectStreamError extends Error {
  readonly offset: number;
  constructor(message: string, offset: number) {
    super(`${message} at offset ${offset}`);
    this.name = "ObjectStreamError";
    this.offset = offset;
  }
}

export function objectKindFromTag(tag: number): ObjectKind | undefined {
  return TAG_KIND[tag];
}

export function objectKindToTag(kind: ObjectKind): number {
  const tag = OBJECT_KIND_TAG[kind];
  if (tag === undefined) throw new RangeError(`unknown object kind ${String(kind)}`);
  return tag;
}

/** Encode one frame. */
export function encodeObjectFrame(frame: ObjectFrame): Uint8Array {
  const len = frame.payload.byteLength;
  if (len > 0xffff_ffff) throw new RangeError("object payload exceeds u32 length");
  const out = new Uint8Array(5 + len);
  out[0] = objectKindToTag(frame.kind);
  new DataView(out.buffer).setUint32(1, len, false);
  out.set(frame.payload, 5);
  return out;
}

/** Encode a sequence of frames into one object stream. */
export function encodeObjectStream(frames: Iterable<ObjectFrame>): Uint8Array {
  const parts: Uint8Array[] = [];
  let total = 0;
  for (const f of frames) {
    const enc = encodeObjectFrame(f);
    parts.push(enc);
    total += enc.byteLength;
  }
  const out = new Uint8Array(total);
  let pos = 0;
  for (const p of parts) {
    out.set(p, pos);
    pos += p.byteLength;
  }
  return out;
}

/** Lazily decode an object stream. Payloads are views into `bytes` (no copy). */
export function* iterateObjectStream(bytes: Uint8Array): Generator<ObjectFrame, void, undefined> {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let pos = 0;
  while (pos < bytes.byteLength) {
    if (bytes.byteLength - pos < 5) throw new ObjectStreamError("truncated object stream header", pos);
    const tag = bytes[pos] as number;
    const kind = TAG_KIND[tag];
    if (kind === undefined) throw new ObjectStreamError(`bad object kind ${tag}`, pos);
    const len = view.getUint32(pos + 1, false);
    if (bytes.byteLength - pos - 5 < len) throw new ObjectStreamError("truncated object stream payload", pos);
    yield { kind, payload: bytes.subarray(pos + 5, pos + 5 + len) };
    pos += 5 + len;
  }
}

/** Decode a whole object stream into an array of frames. */
export function decodeObjectStream(bytes: Uint8Array): ObjectFrame[] {
  return Array.from(iterateObjectStream(bytes));
}
