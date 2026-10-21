// The framing of the client socket (ADR 0018): a u32 little-endian length,
// then that many bytes of one UTF-8 JSON object. Mirrors
// crates/fabric-client/src/frame.rs byte for byte.
//
// A JavaScript string may hold a lone UTF-16 surrogate, which JSON.stringify
// writes as an escape and JSON.parse reads back, but which is not Unicode
// text and which the Rust reader refuses. Neither direction lets one
// through, in a value or in a key.

import { FrameEmptyError, FrameTooLargeError, MalformedError } from "./errors.ts";
import { isUnicodeText } from "./schema.ts";

/** The largest frame either side sends or reads: 16 MiB. */
export const MAX_FRAME = 16 << 20;

const HEADER = 4;
const encoder = new TextEncoder();
// Fatal, so bytes that are not UTF-8 fail the frame instead of turning into U+FFFD.
const decoder = new TextDecoder("utf-8", { fatal: true });

/** The bytes of `value` as one frame: its length, then its JSON. */
export function encodeFrame(value: unknown): Uint8Array {
  if (!isUnicodeJson(value)) {
    throw new MalformedError(LONE_SURROGATE);
  }
  const body = encoder.encode(JSON.stringify(value));
  if (body.length > MAX_FRAME) {
    throw new FrameTooLargeError(body.length, MAX_FRAME);
  }
  const frame = new Uint8Array(HEADER + body.length);
  new DataView(frame.buffer).setUint32(0, body.length, true);
  frame.set(body, HEADER);
  return frame;
}

/** The JSON value a frame's body holds. */
export function decodeJson(body: Uint8Array): unknown {
  let text: string;
  try {
    text = decoder.decode(body);
  } catch (error) {
    throw new MalformedError("not UTF-8", error);
  }
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (error) {
    throw new MalformedError("not JSON", error);
  }
  if (!isUnicodeJson(value)) {
    throw new MalformedError(LONE_SURROGATE);
  }
  return value;
}

const LONE_SURROGATE = "a string holds a lone UTF-16 surrogate, which is not Unicode text";

/** Whether every string in `value`, keys included, is Unicode text. */
function isUnicodeJson(value: unknown): boolean {
  if (typeof value === "string") {
    return isUnicodeText(value);
  }
  if (Array.isArray(value)) {
    return value.every(isUnicodeJson);
  }
  if (typeof value === "object" && value !== null) {
    return Object.entries(value).every(([key, field]) => isUnicodeText(key) && isUnicodeJson(field));
  }
  return true;
}

/** An incremental reader of frames from a byte stream. */
export type FrameDecoder = {
  /** Appends bytes as they arrive. */
  push(chunk: Uint8Array): void;
  /**
   * The next whole frame's body, or `undefined` until it has arrived.
   * Throws as soon as a header declares 0 bytes or more than the cap,
   * before any of that body is waited for.
   */
  next(): Uint8Array | undefined;
  /** How many bytes are held that are not yet a whole frame. */
  buffered(): number;
};

/** A decoder that holds the bytes of at most one partial frame plus what followed it. */
export function createFrameDecoder(): FrameDecoder {
  // A list of chunks, so a 16 MiB frame in many small reads is copied once.
  const chunks: Uint8Array[] = [];
  let size = 0;

  function take(count: number): Uint8Array {
    const out = new Uint8Array(count);
    let filled = 0;
    while (filled < count) {
      const head = chunks[0];
      if (head === undefined) {
        throw new Error("frame decoder: took more bytes than it holds");
      }
      const used = Math.min(head.length, count - filled);
      out.set(head.subarray(0, used), filled);
      filled += used;
      if (used === head.length) {
        chunks.shift();
      } else {
        chunks[0] = head.subarray(used);
      }
    }
    size -= count;
    return out;
  }

  function declaredLength(): number {
    const header = new Uint8Array(HEADER);
    let filled = 0;
    for (const chunk of chunks) {
      const used = Math.min(chunk.length, HEADER - filled);
      header.set(chunk.subarray(0, used), filled);
      filled += used;
      if (filled === HEADER) {
        break;
      }
    }
    return new DataView(header.buffer).getUint32(0, true);
  }

  return {
    push(chunk) {
      if (chunk.length > 0) {
        chunks.push(chunk);
        size += chunk.length;
      }
    },
    next() {
      if (size < HEADER) {
        return undefined;
      }
      const length = declaredLength();
      if (length === 0) {
        throw new FrameEmptyError();
      }
      if (length > MAX_FRAME) {
        throw new FrameTooLargeError(length, MAX_FRAME);
      }
      if (size < HEADER + length) {
        return undefined;
      }
      take(HEADER);
      return take(length);
    },
    buffered() {
      return size;
    },
  };
}
