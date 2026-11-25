import { expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fixture } from "./server";

test("role cookies are independent and creator page enforces permission", async () => {
  const server = fixture(0);
  const url = server.url.origin;
  const signIn = (role: string) => fetch(url + "/login", {
    method: "POST", headers: { Origin: url }, body: new URLSearchParams({ role }), redirect: "manual",
  });
  try {
    const creator = (await signIn("creator")).headers.get("set-cookie")!.split(";")[0];
    const viewer = (await signIn("viewer")).headers.get("set-cookie")!.split(";")[0];
    const creatorPage = await fetch(url + "/creator", { headers: { Cookie: creator } });
    expect(creatorPage.status).toBe(200);
    expect((await fetch(url + "/creator", { headers: { Cookie: viewer } })).status).toBe(403);
    const logout = await fetch(url + "/logout", {
      method: "POST", headers: { Origin: url, Cookie: viewer }, redirect: "manual",
    });
    expect(logout.headers.get("set-cookie")).toContain("Max-Age=0");
    expect(await (await fetch(url, { headers: { Cookie: creator } })).text()).toContain("Signed in as Creator");
  } finally {
    await server.stop(true);
  }
});

test("the second origin ignores synthetic login cookies", async () => {
  const server = fixture(0, true);
  try {
    const response = await fetch(server.url, { headers: { Cookie: "slide_fixture_role=creator" } });
    expect(await response.text()).toContain("const role = null");
  } finally {
    await server.stop(true);
  }
});

test("the QA-only fixture hides links to the absent second origin", async () => {
  const server = fixture(0, false, null);
  try {
    const html = await (await fetch(new URL("/qa", server.url))).text();
    expect(html).toContain("QA fixture lab");
    expect(html).not.toContain(">Second origin<");
  } finally {
    await server.stop(true);
  }
});

test("the fingerprint fixture is local to the primary fixture origin", async () => {
  const primary = fixture(0);
  const secondary = fixture(0, true);
  try {
    const response = await fetch(new URL("/qa/fingerprint", primary.url));
    expect(response.status).toBe(200);
    expect(await response.text()).toContain("canvasHash");
    expect((await fetch(new URL("/qa/fingerprint", secondary.url))).status).toBe(404);
  } finally {
    await primary.stop(true);
    await secondary.stop(true);
  }
});

const fingerprintBrowser = process.env.SLIDE_TEST_FINGERPRINT_BROWSER;
test.skipIf(!fingerprintBrowser)("patched Chromium applies workspace fingerprint settings to the fixture", async () => {
  const server = fixture(0);
  const root = await mkdtemp(join(tmpdir(), "slide-fingerprint-test-"));
  async function read(seed: number, cores: number) {
    const browser = Bun.spawn([
      fingerprintBrowser!, "--headless", "--no-first-run", "--disable-background-networking",
      "--use-mock-keychain", `--user-data-dir=${join(root, String(seed))}`,
      `--fingerprint=${seed}`, `--fingerprint-hardware-concurrency=${cores}`, "--timezone=UTC",
      "--fingerprint-platform=windows", new URL("/qa/fingerprint", server.url).href, "--dump-dom",
    ], { stdout: "pipe", stderr: "ignore" });
    const result = Promise.all([new Response(browser.stdout).text(), browser.exited]);
    const timeout = setTimeout(() => browser.kill(), 15_000);
    try {
      const [html, status] = await result;
      expect(status).toBe(0);
      const output = html.match(/<pre id="readings">([\s\S]+?)<\/pre>/);
      expect(output).not.toBeNull();
      return JSON.parse(output![1]) as { cores: number; timezone: string; platform: string; canvasHash: string };
    } finally {
      clearTimeout(timeout);
    }
  }
  try {
    const a = await read(12345, 7);
    const b = await read(54321, 11);
    expect(a).toMatchObject({ cores: 7, timezone: "UTC", platform: "Win32" });
    expect(b).toMatchObject({ cores: 11, timezone: "UTC", platform: "Win32" });
    expect(a.canvasHash).not.toBe(b.canvasHash);
  } finally {
    await server.stop(true);
    await rm(root, { recursive: true, force: true });
  }
}, 35_000);

test("the owned fixture round-trips synthetic cookies in both formats and refuses unsupported attributes", async () => {
  const server = fixture(0);
  const url = new URL("/qa/cookie", server.url);
  const imported = (format: string, contents: string, origin = server.url.origin) => {
    const body = new FormData();
    body.set("format", format);
    body.set("cookie", new File([contents], "cookie.txt"));
    return fetch(url, { method: "POST", headers: { Origin: origin }, body, redirect: "manual" });
  };
  try {
    for (const format of ["json", "netscape"]) {
      url.searchParams.set("format", format);
      const exported = await fetch(url, { headers: { Cookie: "slide_fixture_role=creator; unrelated=private" } });
      expect(exported.headers.get("cache-control")).toBe("no-store");
      expect(exported.headers.get("content-disposition")).toContain("attachment");
      const contents = await exported.text();
      expect(contents).not.toContain("unrelated");
      const response = await imported(format, contents);
      expect(response.status).toBe(303);
      const cookie = response.headers.get("set-cookie")!;
      expect(cookie).toBe("slide_fixture_role=creator; Path=/; HttpOnly; SameSite=Strict");
      expect((await fetch(new URL("/creator", server.url), { headers: { Cookie: cookie.split(";")[0] } })).status).toBe(200);
      expect((await imported(format, contents, "http://127.0.0.1:1")).status).toBe(403);
      expect((await imported(format, contents.replace("127.0.0.1", "example.test"))).status).toBe(400);
      if (format === "json") {
        const modified = JSON.parse(contents);
        modified[0].secure = true;
        expect((await imported(format, JSON.stringify(modified))).status).toBe(400);
      }
    }
    const html = await (await fetch(new URL("/qa", server.url))).text();
    expect(html).toContain("Fixture-only regional and device simulation");
    for (const script of html.matchAll(/<script>([\s\S]*?)<\/script>/g)) new Function(script[1]);
  } finally {
    await server.stop(true);
  }
});
