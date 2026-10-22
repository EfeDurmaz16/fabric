// The framing of the client socket (ADR 0018): a u32 little-endian length,
// then that many bytes of one UTF-8 JSON object. Mirrors
// crates/fabric-client/src/frame.rs byte for byte.
//
// A JavaScript string may hold a lone UTF-16 surrogate, which JSON.stringify
// writes as an escape and JSON.parse reads back, but which is not Unicode
// text and which the Rust reader refuses. Neither direction lets one
// through, in a value or in a key.
//
// JSON.parse also reads an integer written as 2.0, 1e0 or -0, which
// serde_json reads as a float that no field of the client API takes, and
// TextDecoder drops a leading byte order mark, which serde_json refuses.
// The reader refuses both where the Rust reader does (review finding on
// PR #13).

import { FrameEmptyError, FrameTooLargeError, MalformedError } from "./errors.ts";
import { isUnicodeText } from "./schema.ts";

/** The largest frame either side sends or reads: 16 MiB. */
export const MAX_FRAME = 16 << 20;

const HEADER = 4;
const encoder = new TextEncoder();
// Fatal, so bytes that are not UTF-8 fail the frame instead of turning into U+FFFD.
// A leading byte order mark stays U+FEFF, which JSON.parse refuses as
// serde_json does, instead of being dropped before the parse.
const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

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

/**
 * The JSON value a frame's body holds. A number written with a fraction,
 * an exponent or as `-0` reads as a symbol that every schema refuses.
 */
export function decodeJson(body: Uint8Array): unknown {
  let text: string;
  try {
    text = decoder.decode(body);
  } catch (error) {
    throw new MalformedError("not UTF-8", error);
  }
  let value: unknown;
  try {
    value = MAYBE_NOT_AN_INTEGER.test(text) ? JSON.parse(text, markNonIntegers) : JSON.parse(text);
  } catch (error) {
    throw new MalformedError("not JSON", error);
  }
  if (!isUnicodeJson(value)) {
    throw new MalformedError(LONE_SURROGATE);
  }
  return value;
}

const LONE_SURROGATE = "a string holds a lone UTF-16 surrogate, which is not Unicode text";

/**
 * What a number written with a fraction, an exponent or as `-0` reads as.
 * serde_json reads each of them as a float, and every number of the client
 * API is an integer, so the Rust readers refuse one wherever the message
 * reads a number and skip it in a field they do not know. A symbol is no
 * JSON value, so every schema refuses it in the same places and drops it
 * with an unknown field, where a number would pass `int()`.
 */
const NOT_AN_INTEGER = Symbol("a number not written as an integer");

/** The one form serde_json reads as an integer: no fraction, no exponent, no `-0`. */
const INTEGER = /^(?:0|-?[1-9][0-9]*)$/;

/**
 * Matches every JSON text holding a number with a fraction or an exponent,
 * whose digits run into `.`, `e` or `E`, or a `-0`; a match inside a string
 * only costs the slower parse. Most frames hold none, so they skip the
 * reviver, which parses several times slower.
 */
const MAYBE_NOT_AN_INTEGER = /[0-9][.eE]|-0/;

/**
 * A reviver that reads a number not written as an integer as
 * `NOT_AN_INTEGER`. Node 22 and Bun hand a reviver the source text of each
 * number, which the version JSON.parse made of it no longer tells apart.
 */
function markNonIntegers(_key: string, value: unknown, context?: { readonly source?: string }): unknown {
  return typeof value === "number" && context?.source !== undefined && !INTEGER.test(context.source)
    ? NOT_AN_INTEGER
    : value;
}

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
