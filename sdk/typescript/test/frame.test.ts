// The framing codec: the cap and the empty frame are refused from the
// header alone, and frames come out whole however the stream splits them.

import { describe, expect, test } from "bun:test";

import { createFrameDecoder, encodeFrame, FrameEmptyError, FrameTooLargeError, MAX_FRAME } from "../src/index.ts";

function header(length: number): Uint8Array {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, length, true);
  return bytes;
}

function text(bytes: Uint8Array | undefined): string | undefined {
  return bytes === undefined ? undefined : new TextDecoder().decode(bytes);
}

describe("the frame decoder", () => {
  // A reader that waited for the body first could be made to hold 4 GiB.
  test("refuses a length over the cap from the header alone", () => {
    const decoder = createFrameDecoder();
    decoder.push(header(MAX_FRAME + 1));
    expect(() => decoder.next()).toThrow(FrameTooLargeError);
  });

  test("accepts a length of exactly the cap", () => {
    const decoder = createFrameDecoder();
    decoder.push(header(MAX_FRAME));
    expect(decoder.next()).toBeUndefined();
  });

  test("refuses a length of zero", () => {
    const decoder = createFrameDecoder();
    decoder.push(header(0));
    expect(() => decoder.next()).toThrow(FrameEmptyError);
  });

  test("reads a frame split across chunks, one byte at a time", () => {
    const frame = encodeFrame({ op: "job_list" });
    const decoder = createFrameDecoder();
    for (const byte of frame.subarray(0, -1)) {
      decoder.push(Uint8Array.of(byte));
      expect(decoder.next()).toBeUndefined();
    }
    decoder.push(frame.subarray(-1));
    expect(text(decoder.next())).toBe('{"op":"job_list"}');
    expect(decoder.buffered()).toBe(0);
  });

  test("reads several frames from one chunk and keeps the partial one after them", () => {
    const first = encodeFrame({ n: 1 });
    const second = encodeFrame({ n: 2 });
    const third = encodeFrame({ n: 3 });
    const chunk = new Uint8Array([...first, ...second, ...third.subarray(0, 5)]);
    const decoder = createFrameDecoder();
    decoder.push(chunk);
    expect(text(decoder.next())).toBe('{"n":1}');
    expect(text(decoder.next())).toBe('{"n":2}');
    expect(decoder.next()).toBeUndefined();
    decoder.push(third.subarray(5));
    expect(text(decoder.next())).toBe('{"n":3}');
    expect(decoder.next()).toBeUndefined();
  });
});

describe("the frame encoder", () => {
  test("writes the length little-endian before the UTF-8 JSON", () => {
    expect([...encodeFrame("é")]).toStrictEqual([4, 0, 0, 0, 0x22, 0xc3, 0xa9, 0x22]);
  });
});
