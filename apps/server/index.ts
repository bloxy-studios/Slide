import { SQL } from "bun";
import { createApi, migrate } from "./api";
import { objectStorage } from "./storage";
import { MAX_ENVELOPE_BYTES } from "../../packages/schemas/sync";

if (!process.env.DATABASE_URL || !process.env.S3_ENDPOINT || !process.env.S3_BUCKET ||
    !process.env.S3_ACCESS_KEY_ID || !process.env.S3_SECRET_ACCESS_KEY) {
  throw new Error("Configure DATABASE_URL and S3_ENDPOINT, S3_BUCKET, S3_ACCESS_KEY_ID and S3_SECRET_ACCESS_KEY.");
}
const db = new SQL(process.env.DATABASE_URL);
await migrate(db);
const server = Bun.serve({
  hostname: process.env.SLIDE_HOST ?? "127.0.0.1",
  port: Number(process.env.SLIDE_PORT ?? 4321),
  maxRequestBodySize: MAX_ENVELOPE_BYTES + 4096,
  fetch: createApi(db, objectStorage()),
});
console.log(`Slide local sync API listening on ${server.url}`);
