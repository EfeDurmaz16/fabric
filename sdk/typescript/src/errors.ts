// Why a call did not get its reply, mirroring `ClientError` in
// crates/fabric-client/src/client.rs. Each failure is its own class so a
// caller can branch with `instanceof` and read typed fields, never the
// message text.

import type { Op, Reason, Refusal, RefusalKind } from "./schema.ts";

/** The base of every error this client throws. */
export class FabricError extends Error {
  override name = "FabricError";
}

/** No node listens on the socket. */
export class NoDaemonError extends FabricError {
  override name = "NoDaemonError";
  readonly socketPath: string;

  constructor(socketPath: string, cause: unknown) {
    super(`no fabric daemon at ${socketPath} (${describe(cause)}); start one with \`fabric serve\``, {
      cause,
    });
    this.socketPath = socketPath;
  }
}

/** The state directory or the socket is not this user's alone. */
export class UntrustedError extends FabricError {
  override name = "UntrustedError";
  readonly path: string;
  readonly reason: string;

  constructor(path: string, reason: string) {
    super(`${path} ${reason}, so its answers are not trusted; it must be yours and mode 0700`);
    this.path = path;
    this.reason = reason;
  }
}

/** A frame could not be written or read. */
export class FrameError extends FabricError {
  override name = "FrameError";
}

/** The socket failed, or closed inside a frame. */
export class FrameIoError extends FrameError {
  override name = "FrameIoError";

  constructor(cause: unknown) {
    super(`control socket: ${describe(cause)}`, { cause });
  }
}

/** A frame declares, or would need, more bytes than the cap. */
export class FrameTooLargeError extends FrameError {
  override name = "FrameTooLargeError";
  readonly length: number;

  constructor(length: number, cap: number) {
    super(`control message of ${length} bytes exceeds the cap of ${cap}`);
    this.length = length;
  }
}

/** A frame declares no bytes, which holds no JSON object. */
export class FrameEmptyError extends FrameError {
  override name = "FrameEmptyError";

  constructor() {
    super("control message is empty");
  }
}

/** A frame is not UTF-8 JSON, or not the message expected at this point. */
export class MalformedError extends FrameError {
  override name = "MalformedError";

  constructor(detail: string, cause?: unknown) {
    super(`control message does not parse: ${detail}`, { cause });
  }
}

/** The node speaks another api version. */
export class IncompatibleError extends FabricError {
  override name = "IncompatibleError";
  readonly client: number;
  readonly node: number;
  readonly nodeVersion: string;

  constructor(client: number, node: number, nodeVersion: string) {
    super(
      `the node speaks client api ${node} (fabric ${nodeVersion}) and this client speaks ${client}; use a client and a node from the same release`,
    );
    this.client = client;
    this.node = node;
    this.nodeVersion = nodeVersion;
  }
}

/** The node does not list the op among its capabilities, so nothing was sent. */
export class UnsupportedError extends FabricError {
  override name = "UnsupportedError";
  readonly op: Op;

  constructor(op: Op) {
    super(`the node does not serve \`${op}\`; it runs an older fabric`);
    this.op = op;
  }
}

/** The node closed the connection before it welcomed the client. */
export class ClosedBeforeWelcomeError extends FabricError {
  override name = "ClosedBeforeWelcomeError";
  readonly socketPath: string;

  constructor(socketPath: string, cause?: unknown) {
    super(
      `the node at ${socketPath} closed the connection before it answered the hello; it may run a fabric older than the client API`,
      { cause },
    );
    this.socketPath = socketPath;
  }
}

/** The node closed the connection before the final answer. */
export class ClosedBeforeAnswerError extends FabricError {
  override name = "ClosedBeforeAnswerError";
  readonly op: Op;

  constructor(op: Op) {
    super(`the node closed the connection before it answered \`${op}\``);
    this.op = op;
  }
}

/** The node answered a request with the reply of another op. */
export class UnexpectedError extends FabricError {
  override name = "UnexpectedError";
  readonly op: Op;
  readonly got: Op;

  constructor(op: Op, got: Op) {
    super(`the node answered \`${op}\` with the reply of \`${got}\``);
    this.op = op;
    this.got = got;
  }
}

/** The node refused or failed the command; `message` is the node's sentence. */
export class RefusedError extends FabricError {
  override name = "RefusedError";
  readonly refusal: Refusal;
  readonly kind: RefusalKind;
  readonly reason: Reason;

  constructor(refusal: Refusal) {
    super(refusal.message);
    this.refusal = refusal;
    this.kind = refusal.kind;
    this.reason = refusal.reason;
  }
}

/**
 * The request does not validate, so it never left the client. Only a
 * caller without the types, or one that cast around them, meets this.
 */
export class InvalidRequestError extends FabricError {
  override name = "InvalidRequestError";

  constructor(detail: string, cause: unknown) {
    super(`the request is not valid: ${detail}`, { cause });
  }
}

function describe(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
