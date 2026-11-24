import { cookieFormat, exportFixtureCookie, importFixtureCookie, qaControls } from "./qa";

const primary = "http://127.0.0.1:4310";
const secondary = "http://127.0.0.1:4311";
export const qaPort = 4320;
const roles = { creator: "Creator", viewer: "Viewer" } as const;
type Role = keyof typeof roles;

export function fixture(port = 4310, isolated = false) {
  return Bun.serve({
    hostname: "127.0.0.1",
    port,
    maxRequestBodySize: 8192,
    async fetch(request, server) {
      const url = new URL(request.url);
      if (url.host !== `127.0.0.1:${server.port}`) return new Response("Invalid host", { status: 400 });
      const headers = { "Cache-Control": "no-store", "Content-Type": "text/html; charset=utf-8" };
      const cookie = request.headers.get("Cookie")?.match(/(?:^|;\s*)slide_fixture_role=(creator|viewer)(?:;|$)/)?.[1] as Role | undefined;
      if (!isolated && url.pathname === "/qa/cookie") {
        if (request.method === "GET") {
          const format = url.searchParams.get("format");
          if (!cookie || !cookieFormat(format)) return new Response("Sign in to the fixture and choose JSON or Netscape.", { status: 400 });
          return new Response(exportFixtureCookie(cookie, format), { headers: {
            "Cache-Control": "no-store", "Content-Type": format === "json" ? "application/json" : "text/plain",
            "Content-Disposition": `attachment; filename="slide-fixture-cookie.${format === "json" ? "json" : "txt"}"`,
          } });
        }
        if (request.method === "POST") {
          if (request.headers.get("Origin") !== url.origin) return new Response("Invalid origin", { status: 403 });
          try {
            const form = await request.formData();
            const format = form.get("format"), file = form.get("cookie");
            if (typeof format !== "string" || !cookieFormat(format) || !(file instanceof File) || file.size > 4096)
              return new Response("Choose a synthetic JSON or Netscape file of at most 4 KiB.", { status: 400 });
            const role = importFixtureCookie(await file.text(), format);
            return new Response(null, { status: 303, headers: {
              Location: "/qa",
              "Set-Cookie": `slide_fixture_role=${role}; Path=/; HttpOnly; SameSite=Strict`,
            } });
          } catch (error) {
            return new Response(error instanceof Error ? error.message : "Invalid cookie fixture.", { status: 400 });
          }
        }
        return new Response("Method not allowed", { status: 405 });
      }
      if (request.method === "POST" && !isolated && ["/login", "/logout"].includes(url.pathname)) {
        if (request.headers.get("Origin") !== url.origin) return new Response("Invalid origin", { status: 403 });
        const role = url.pathname === "/login" ? (await request.formData()).get("role") : null;
        if (url.pathname === "/login" && role !== "creator" && role !== "viewer") return new Response("Unknown role", { status: 400 });
        return new Response(null, { status: 303, headers: {
          Location: "/",
          "Set-Cookie": `slide_fixture_role=${role ?? ""}; Path=/; HttpOnly; SameSite=Strict; Max-Age=${role ? 604800 : 0}`,
        } });
      }
      if (request.method !== "GET") return new Response("Method not allowed", { status: 405 });
      const route = request.headers.get("X-Slide-QA-Proxy") === "loopback-fixture" ? "proxy" : "direct";
      if (!isolated && url.pathname === "/route") {
        return new Response(`Slide QA route: ${route}\n`, { headers: { ...headers, "Content-Type": "text/plain; charset=utf-8", "X-Slide-Fixture-Route": route } });
      }
      if (!["/", "/creator", "/bookmarks", "/qa"].includes(url.pathname) || isolated && url.pathname !== "/") {
        return new Response("Not found", { status: 404 });
      }
      const denied = url.pathname === "/creator" && cookie !== "creator";
      const content = isolated
        ? `<h1>Second origin</h1><p>This origin reads its own DOM storage. Cookies are host scoped, not port scoped; this server ignores the login cookie.</p><a href="${primary}">Back to login origin</a>`
        : url.pathname === "/creator"
          ? `<h1>${denied ? "Creator permission required" : "Creator-only fixture"}</h1><a href="/">Back to fixture</a>`
          : url.pathname === "/qa"
            ? '<h1>Slide QA fixture lab</h1><p>Every tool on this page is scoped to this local synthetic fixture.</p>'
          : url.pathname === "/bookmarks"
            ? '<h1>Bookmark this fixture</h1><ul><li><a href="/">Fixture home / start URL</a></li><li><a href="/creator">Creator-only page</a></li></ul>'
            : cookie
              ? `<h1>Signed in as ${roles[cookie]}</h1><form method="post" action="/logout"><button>Log out</button></form>`
              : '<h1>Sign in to the fixture</h1><form method="post" action="/login"><button name="role" value="creator">Sign in as Creator</button><button name="role" value="viewer">Sign in as Viewer</button></form>';
      const role = isolated ? null : cookie ?? null;
      return new Response(`<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
        <title>Slide Workspaces • Test Fixture</title>
        <style>
        :root{color-scheme:light dark}body{font:16px system-ui;margin:0;padding:5vh 5vw;background:Canvas;color:CanvasText}
        main{max-width:42rem;margin:auto;border:1px solid GrayText;border-radius:16px;padding:2rem}
        header{font-size:.8rem;text-transform:uppercase;letter-spacing:.12em;color:GrayText}
        button{cursor:pointer;font:inherit;padding:.6rem .9rem;margin:0 .5rem .5rem 0}
        nav{display:flex;flex-wrap:wrap;gap:1rem;margin-top:1.5rem}dd{margin-bottom:.7rem}code{font-size:1rem}
        </style>
        <main><header>Slide Workspaces / Synthetic fixture</header>${content}
        <p>Fixture route: <strong>${route}</strong></p>
        <h2>Storage markers</h2>
        <dl><dt>Cookie identity</dt><dd><code>${role ?? "signed out"}</code></dd>
        <dt>localStorage</dt><dd><code id="local">Checking…</code></dd>
        <dt>IndexedDB</dt><dd><code id="indexed">Checking…</code></dd></dl>
        ${isolated ? "" : `<nav><a href="/">Home</a><a href="/creator">Creator-only page</a><a href="/bookmarks">Bookmarks</a><a href="${secondary}">Second origin</a><a href="/qa">QA fixture lab</a></nav>`}
        ${url.pathname === "/qa" ? qaControls : ""}
        </main>
        <script>
        const role = ${JSON.stringify(role)};
        if (role && !localStorage.getItem("slide-marker")) localStorage.setItem("slide-marker", "fixture:" + role);
        const marker = localStorage.getItem("slide-marker") || "(not set)";
        document.querySelector("#local").textContent = marker;
        const db = indexedDB.open("slide-fixture", 1);
        db.onupgradeneeded = () => db.result.createObjectStore("markers");
        db.onerror = () => document.querySelector("#indexed").textContent = "Unavailable";
        db.onsuccess = () => {
          const tx = db.result.transaction("markers", role ? "readwrite" : "readonly");
          const store = tx.objectStore("markers");
          const request = store.get("role");
          request.onsuccess = () => {
            if (request.result === undefined && role) store.put("fixture:" + role, "role");
            tx.oncomplete = () => {
              document.querySelector("#indexed").textContent = request.result ?? (role ? "fixture:" + role : "(not set)");
              db.result.close();
            };
          };
        };
        </script></html>`, { status: denied ? 403 : 200, headers });
    },
  });
}

if (import.meta.main) {
  if (!process.argv.includes("--qa-only")) {
    fixture();
    fixture(4311, true);
    console.log(`Synthetic fixture: ${primary} (second origin: ${secondary})`);
  }
  fixture(qaPort);
  console.log(`Synthetic route fixture: http://127.0.0.1:${qaPort}/qa`);
}
