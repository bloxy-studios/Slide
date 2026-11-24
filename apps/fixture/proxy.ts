import { createServer, request } from "node:http";
import { qaPort, secondaryPort } from "./server";

export function fixtureProxy(port = 4312, targetPort = qaPort, authenticate = false, secondPort = secondaryPort) {
  const expected = "Basic " + Buffer.from("fixture:fixture").toString("base64");
  const destinations = new Map([targetPort, secondPort].map(port => [`http://127.0.0.1:${port}`, port]));
  return createServer((incoming, reply) => {
    let target: URL;
    try {
      target = new URL(incoming.url ?? "");
    } catch {
      reply.writeHead(400).end("Use an absolute fixture URL");
      return;
    }
    const destination = destinations.get(target.origin);
    if (destination === undefined || target.username || target.password ||
        !["GET", "POST"].includes(incoming.method ?? "")) {
      reply.writeHead(403).end("This proxy only serves the local Slide fixture");
      return;
    }
    if (authenticate && incoming.headers["proxy-authorization"] !== expected) {
      reply.writeHead(407, { "Proxy-Authenticate": 'Basic realm="Slide synthetic QA"' }).end("Test proxy credentials required");
      return;
    }
    const headers: Record<string, string> = { "X-Slide-QA-Proxy": "loopback-fixture" };
    for (const key of ["cookie", "origin", "content-type", "content-length"]) {
      const value = incoming.headers[key];
      if (typeof value === "string") headers[key] = value;
    }
    const upstream = request({ hostname: "127.0.0.1", port: destination, path: target.pathname + target.search,
      method: incoming.method, headers }, response => {
      reply.writeHead(response.statusCode ?? 502, response.headers);
      response.pipe(reply);
    });
    upstream.setTimeout(2000, () => upstream.destroy());
    upstream.on("error", () => { if (!reply.headersSent) reply.writeHead(502); reply.end(); });
    incoming.on("error", () => upstream.destroy());
    incoming.pipe(upstream);
  }).listen(port, "127.0.0.1");
}

if (import.meta.main) {
  const authenticated = process.argv.includes("--auth");
  fixtureProxy(4312, qaPort, authenticated);
  console.log(`Loopback-only fixture HTTP proxy on 127.0.0.1:4312 (${authenticated ? "synthetic Basic auth required" : "no auth"})`);
}
