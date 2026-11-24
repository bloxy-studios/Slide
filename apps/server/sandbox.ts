import { createHmac, timingSafeEqual } from "node:crypto";

export type Plan = { id: string; monthlyCents: number; profiles: number; seats: number };
export type BillingEvent = {
  id: string; account: string; sequence: number; plan: string; status: "paid" | "failed";
};
export type SignedEvent = { payload: string; signature: string };
export interface BillingAdapter {
  readonly mode: "sandbox" | "unavailable";
  charge(account: string, plan: string, key: string, outcome?: BillingEvent["status"]): SignedEvent;
}
export interface NetworkProvider {
  readonly mode: "sandbox" | "unavailable";
  locations(): readonly { id: string; priceCents: number }[];
  balance(account: string): number;
  quote(location: string): number;
  purchase(account: string, location: string, quotedCents: number, key: string): { id: string; cents: number };
}

export const samplePlans: readonly Plan[] = [
  { id: "starter", monthlyCents: 0, profiles: 5, seats: 1 },
  { id: "team", monthlyCents: 1500, profiles: 50, seats: 10 },
];

function remember<T>(entries: Map<string, { fingerprint: string; value: T }>, account: string, key: string,
    fingerprint: string, create: () => T): T {
  if (!key.trim()) throw new Error("An idempotency key is required.");
  const index = JSON.stringify([account, key]);
  const previous = entries.get(index);
  if (previous) {
    if (previous.fingerprint !== fingerprint) throw new Error("Idempotency key already used for a different operation.");
    return previous.value;
  }
  const value = create();
  entries.set(index, { fingerprint, value });
  return value;
}

export class SandboxBilling implements BillingAdapter {
  readonly mode = "sandbox";
  private readonly operations = new Map<string, { fingerprint: string; value: SignedEvent }>();
  private readonly sequences = new Map<string, number>();
  private readonly received = new Set<string>();
  private readonly entitlements = new Map<string, BillingEvent>();
  private readonly history: BillingEvent[] = [];

  constructor(private readonly signingKey: string, private readonly plans: readonly Plan[] = samplePlans) {}

  private sign(payload: string) { return createHmac("sha256", this.signingKey).update(payload).digest("hex"); }

  charge(account: string, plan: string, key: string, outcome: BillingEvent["status"] = "paid"): SignedEvent {
    if (!this.plans.some(row => row.id === plan)) throw new Error("Unknown sandbox plan.");
    return remember(this.operations, account, key, JSON.stringify([plan, outcome]), () => {
      const sequence = (this.sequences.get(account) ?? 0) + 1;
      this.sequences.set(account, sequence);
      const payload = JSON.stringify({ id: JSON.stringify([account, sequence]), account, sequence, plan, status: outcome });
      return Object.freeze({ payload, signature: this.sign(payload) });
    });
  }

  deliver(event: SignedEvent): "applied" | "failed" | "duplicate" | "stale" {
    if (!/^[a-f0-9]{64}$/.test(event.signature) ||
        !timingSafeEqual(Buffer.from(event.signature, "hex"), Buffer.from(this.sign(event.payload), "hex")))
      throw new Error("Invalid sandbox webhook signature.");
    const row = JSON.parse(event.payload) as BillingEvent;
    if (!row || !Number.isSafeInteger(row.sequence) || row.sequence < 1 ||
        typeof row.account !== "string" || row.id !== JSON.stringify([row.account, row.sequence]) ||
        !this.plans.some(plan => plan.id === row.plan) || !["paid", "failed"].includes(row.status))
      throw new Error("Invalid sandbox billing event.");
    if (this.received.has(row.id)) return "duplicate";
    this.received.add(row.id);
    this.history.push(row);
    if (row.sequence <= (this.entitlements.get(row.account)?.sequence ?? 0)) return "stale";
    if (row.status === "failed") return "failed";
    this.entitlements.set(row.account, row);
    return "applied";
  }

  entitlement(account: string) {
    return this.plans.find(plan => plan.id === (this.entitlements.get(account)?.plan ?? this.plans[0]?.id));
  }

  events(account: string) { return this.history.filter(row => row.account === account).map(row => ({ ...row })); }
}

export class SandboxNetworkProvider implements NetworkProvider {
  readonly mode = "sandbox";
  private readonly ledger = new Map<string, { fingerprint: string; value: { id: string; cents: number } }>();
  private readonly spent = new Map<string, number>();
  constructor(private readonly startingCents = 500) {}
  locations() { return [{ id: "fixture-location", priceCents: 100 }]; }
  balance(account: string) { return this.startingCents - (this.spent.get(account) ?? 0); }
  quote(location: string) {
    const locationInfo = this.locations().find(row => row.id === location);
    if (!locationInfo) throw new Error("Unknown sandbox location.");
    return locationInfo.priceCents;
  }
  purchase(account: string, location: string, quotedCents: number, key: string) {
    return remember(this.ledger, account, key, JSON.stringify([location, quotedCents]), () => {
      const cents = this.quote(location);
      if (quotedCents !== cents) throw new Error("Accept the current sandbox quote first.");
      if (this.balance(account) < cents) throw new Error("Insufficient sandbox balance.");
      this.spent.set(account, (this.spent.get(account) ?? 0) + cents);
      return Object.freeze({ id: "sandbox-" + (this.ledger.size + 1), cents });
    });
  }
}
