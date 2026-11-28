// HTTPS carries the same facade JSON as the private socket. The gateway binds
// the authority and takes the credential exclusively from the request header.
import { request } from "node:https";
import type { IncomingMessage } from "node:http";
import { FabricError, FrameTooLargeError } from "./errors.ts";
import { readAnswer, writeClientMessage, type ClientMessage, type FacadeAnswer } from "./facade.ts";
import { decodeJson, MAX_FRAME } from "./frame.ts";

export type GatewayOptions = {
  readonly endpoint: string;
  readonly token: string;
  /** PEM roots to trust for this gateway. Defaults to Node's trusted roots. */
  readonly ca?: string | Buffer;
  /** Total deadline covering connect, TLS, headers and body. Defaults to 35 seconds. */
  readonly timeoutMs?: number;
};

export class GatewayError extends FabricError {
  override name = "GatewayError";
  readonly reason: "http_status" | "timeout" | "transport" | "content_type";
  readonly status: number | undefined;

  constructor(reason: GatewayError["reason"], status?: number) {
    super(status === undefined ? `gateway request failed: ${reason}` : `gateway returned HTTP ${status}`);
    this.reason = reason;
    this.status = status;
  }
}

export function gatewayCaller(options: GatewayOptions): (message: ClientMessage) => Promise<FacadeAnswer> {
  if (!URL.canParse(options.endpoint)) throw new TypeError("invalid facade endpoint URL");
  const endpoint = new URL(options.endpoint);
  if (endpoint.protocol !== "https:" || endpoint.username || endpoint.password ||
      endpoint.pathname !== "/v1/facade" || endpoint.href !== endpoint.origin + "/v1/facade") {
    throw new TypeError("endpoint must be an HTTPS URL ending in /v1/facade without credentials, query or fragment");
  }
  if (!/^[0-9a-f]{64}$/.test(options.token)) throw new TypeError("invalid facade bearer");
  const timeoutMs = options.timeoutMs ?? 35_000;
  if (!Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 2_147_483_647) {
    throw new RangeError("timeoutMs must be a positive 32-bit integer");
  }
  const ca = options.ca;
  return async (message) => {
    const envelope = writeClientMessage(message);
    const size = Buffer.byteLength(JSON.stringify(envelope));
    if (size > 1024 * 1024) throw new FrameTooLargeError(size, 1024 * 1024);
    const { token, ...wire } = envelope;
    const body = Buffer.from(JSON.stringify(wire));
    const signal = AbortSignal.timeout(timeoutMs);
    let response: IncomingMessage | undefined;
    let bytes: Buffer;
    try {
      response = await new Promise<IncomingMessage>((resolve, reject) => {
        const req = request(endpoint, {
          method: "POST", signal, agent: false, rejectUnauthorized: true, minVersion: "TLSv1.2",
          ...(ca === undefined ? {} : { ca }),
          headers: {
            authorization: `Bearer ${token}`, "content-type": "application/json",
            accept: "application/json", "content-length": body.length,
          },
        }, resolve);
        req.on("error", reject);
        req.end(body);
      });
      if (response.statusCode !== 200) throw new GatewayError("http_status", response.statusCode);
      if (response.headers["content-type"]?.split(";")[0]?.trim().toLowerCase() !== "application/json") {
        throw new GatewayError("content_type");
      }
      const declared = Number(response.headers["content-length"]);
      if (declared > MAX_FRAME) throw new FrameTooLargeError(declared, MAX_FRAME);
      const chunks: Buffer[] = [];
      let size = 0;
      for await (const chunk of response as AsyncIterable<Buffer>) {
        size += chunk.length;
        if (size > MAX_FRAME) throw new FrameTooLargeError(size, MAX_FRAME);
        chunks.push(chunk);
      }
      bytes = Buffer.concat(chunks, size);
    } catch (error) {
      if (error instanceof FabricError) throw error;
      throw new GatewayError(signal.aborted ? "timeout" : "transport");
    } finally {
      response?.destroy();
    }
    return readAnswer(decodeJson(bytes));
  };
}
