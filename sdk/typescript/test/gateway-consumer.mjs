// Executed under Node in a clean npm consumer by package.test.ts.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:https";
import { once } from "node:events";
import { randomBytes } from "node:crypto";
import { FabricJobs, GatewayError, FrameTooLargeError, FacadeRefusal, JobId, ProtocolViolation } from "@fabric-jobs/client";

writeFileSync("cert.cnf", [
  "[req]", "distinguished_name=dn", "x509_extensions=cert", "prompt=no", "[dn]", "CN=localhost",
  "[cert]", "subjectAltName=IP:127.0.0.1", "basicConstraints=critical,CA:TRUE",
].join("\n"));
execFileSync("openssl", [
  "req", "-x509", "-config", "cert.cnf", "-newkey", "rsa:2048", "-nodes",
  "-keyout", "key.pem", "-out", "cert.pem", "-days", "1",
], { stdio: "ignore" });
const ca = readFileSync("cert.pem");
const token = randomBytes(32).toString("hex");
const job = JobId.parse("01JABCDEFGHJKMNPQRSTVWXYZ0");
let reply;
const requests = [];
const server = createServer({ key: readFileSync("key.pem"), cert: ca }, async (req, res) => {
  let body = "";
  for await (const chunk of req) body += chunk;
  requests.push(JSON.parse(body));
  assert.equal(req.method, "POST");
  assert.equal(req.url, "/v1/facade");
  assert.equal(req.headers.authorization, `Bearer ${token}`);
  res.setHeader("content-type", "application/json");
  reply(res);
});
server.listen(0, "127.0.0.1");
await once(server, "listening");
try {
  const endpoint = `https://127.0.0.1:${server.address().port}/v1/facade`;
  const jobs = new FabricJobs({ endpoint, token, ca });
  const gatewayError = (reason, status) => (error) =>
    error instanceof GatewayError && error.reason === reason && error.status === status;

  assert.throws(() => new FabricJobs({ endpoint: endpoint.replace("https:", "http:"), token, ca }), TypeError);
  await assert.rejects(new FabricJobs({ endpoint, token }).inspect(job), gatewayError("transport"));
  assert.equal(requests.length, 0);

  reply = res => res.end(JSON.stringify({
    facade: 1, type: "events", job, events: [], next: "41", closed: null, lagged: false,
  }));
  assert.equal((await jobs.watch(job, { after: "41", waitMs: 0 })).next, "41");
  assert.deepEqual(requests.pop(), { facade: 1, type: "watch", job, after: "41", wait_ms: 0 });
  reply = res => res.end(JSON.stringify({
    facade: 1, type: "refused", code: "refused", reason: "not_live", message: "parked",
  }));
  await assert.rejects(jobs.inspect(job), FacadeRefusal);
  reply = res => res.writeHead(307, { location: endpoint }).end();
  await assert.rejects(jobs.inspect(job), gatewayError("http_status", 307));
  assert.equal(requests.length, 2);

  reply = res => res.end("{}");
  await assert.rejects(jobs.inspect(job), ProtocolViolation);
  reply = res => res.writeHead(200, { "content-type": "text/html" }).end();
  await assert.rejects(jobs.inspect(job), gatewayError("content_type"));
  reply = res => res.writeHead(200, { "content-length": 16777217 }).flushHeaders();
  await assert.rejects(jobs.inspect(job), FrameTooLargeError);
  reply = res => {
    res.write(Buffer.alloc(16 * 1024 * 1024));
    res.end(Buffer.from("!"));
  };
  await assert.rejects(jobs.inspect(job), FrameTooLargeError);
  reply = res => res.flushHeaders();
  await assert.rejects(new FabricJobs({ endpoint, token, ca, timeoutMs: 100 }).inspect(job), gatewayError("timeout"));
} finally {
  server.closeAllConnections();
  server.close();
}
