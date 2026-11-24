import { expect, test } from "bun:test";
import { SandboxBilling, SandboxNetworkProvider } from "./sandbox";

test("sandbox payment retries, reordered delivery, failed renewal, and signed upgrade/downgrade", () => {
  const billing = new SandboxBilling("fixture-signing-key");
  const start = billing.charge("alpha", "starter", "start");
  const upgrade = billing.charge("alpha", "team", "upgrade");
  expect(billing.charge("alpha", "team", "upgrade")).toEqual(upgrade);
  expect(() => billing.charge("alpha", "starter", "upgrade")).toThrow("Idempotency");
  expect(() => billing.deliver({ ...upgrade, payload: start.payload })).toThrow("signature");
  expect(billing.deliver(upgrade)).toBe("applied");
  expect(billing.deliver(start)).toBe("stale");
  expect(billing.deliver(upgrade)).toBe("duplicate");
  expect(billing.deliver(billing.charge("alpha", "team", "renew", "failed"))).toBe("failed");
  expect(billing.entitlement("alpha")?.id).toBe("team");
  expect(billing.entitlement("beta")?.id).toBe("starter");
  expect(billing.deliver(billing.charge("alpha", "starter", "downgrade"))).toBe("applied");
  expect(billing.entitlement("alpha")?.id).toBe("starter");
  expect(billing.events("alpha")).toHaveLength(4);
});

test("sandbox provider requires accepted quote and deducts once per idempotency key", () => {
  const provider = new SandboxNetworkProvider(100);
  const location = provider.locations()[0].id;
  expect(() => provider.purchase("alpha", location, 1, "first")).toThrow("quote");
  expect(provider.balance("alpha")).toBe(100);
  const quote = provider.quote(location);
  const order = provider.purchase("alpha", location, quote, "first");
  expect(provider.purchase("alpha", location, quote, "first")).toEqual(order);
  expect(provider.balance("alpha")).toBe(0);
  expect(provider.balance("beta")).toBe(100);
  expect(() => provider.purchase("alpha", location, quote, "another")).toThrow("balance");
});
