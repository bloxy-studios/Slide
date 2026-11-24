import { expect, test } from "bun:test";
import { emptySettings, listRows, processBatch } from "./src/catalog";
import type { Outcome, View, Workspace } from "./src/catalog";

test("filters by folder and all tags before sorting", () => {
  const row = (name: string, tagIds: string[]): Workspace => ({
    id: name, name, status: "closed", detail: null, canClose: false, createdAt: 0, trashedAt: null,
    settings: { ...emptySettings, folderId: "qa", tagIds },
  });
  const view: View = { search: "qa", folder: "qa", tags: ["creator", "active"], status: "", sort: "nameDesc", pageSize: 5, columns: null };
  expect(listRows([row("A", ["active", "creator"]), row("C", ["active"]), row("B", ["active", "creator"])],
    { folders: [{ id: "qa", name: "QA" }], tags: [] }, view, false).map(r => r.name)).toEqual(["B", "A"]);
});

test("batch bounds concurrency and cancellation leaves unstarted work untouched", async () => {
  let active = 0, max = 0, stop = false;
  const outcomes: Outcome[] = [];
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const jobs = Array.from({ length: 100 }, (_, i) => ({
    key: String(i), name: String(i), work: async () => {
      max = Math.max(max, ++active);
      await gate;
      active--;
      return "Done";
    },
  }));
  const batch = processBatch(jobs, 100, () => stop, row => outcomes.push(row));
  stop = true;
  release();
  await batch;
  expect(max).toBe(2);
  expect(outcomes.filter(row => row.state === "Succeeded")).toHaveLength(2);
  expect(outcomes.filter(row => row.state === "Cancelled")).toHaveLength(98);
});
