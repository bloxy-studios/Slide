import { expect, test } from "bun:test";
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
