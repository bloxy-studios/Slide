import { expect, test } from "bun:test";
import { request } from "node:http";
import { fixtureProxy } from "./proxy";
import { fixture } from "./server";

test("fixture proxy authenticates, marks the controlled route and rejects other origins", async () => {
  const server = fixture(0);
  const second = fixture(0, true);
  const proxy = fixtureProxy(0, server.port, true, second.port);
  try {
    await new Promise<void>(resolve => proxy.listening ? resolve() : proxy.once("listening", resolve));
    const address = proxy.address();
    if (!address || typeof address === "string") throw new Error("Expected loopback TCP server");
    const via = (url: string, credentials?: string) => new Promise<[number, string | undefined]>((resolve, reject) => {
      request({ hostname: "127.0.0.1", port: address.port, path: url,
        headers: credentials ? { "Proxy-Authorization": "Basic " + Buffer.from(credentials).toString("base64") } : {} }, response => {
        response.resume().on("end", () => resolve([response.statusCode!, response.headers["x-slide-fixture-route"] as string | undefined]));
      }).on("error", reject).end();
    });
    expect((await fetch(new URL("/route", server.url))).headers.get("X-Slide-Fixture-Route")).toBe("direct");
    expect(await via(new URL("/route", server.url).href, "fixture:wrong")).toEqual([407, undefined]);
    expect(await via(new URL("/route", server.url).href, "fixture:fixture")).toEqual([200, "proxy"]);
    expect(await via("http://127.0.0.1:1/route", "fixture:fixture")).toEqual([403, undefined]);
    expect(await via(second.url.href)).toEqual([407, undefined]);
    expect(await via(second.url.href, "fixture:fixture")).toEqual([200, undefined]);
    expect(await via(new URL("/creator", second.url).href, "fixture:fixture")).toEqual([404, undefined]);
  } finally {
    proxy.close();
    await server.stop(true);
    await second.stop(true);
  }
});
