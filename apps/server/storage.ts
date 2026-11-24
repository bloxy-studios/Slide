import { S3Client } from "bun";

export type BlobStore = {
  put(key: string, bytes: string): Promise<void>;
  get(key: string): Promise<string>;
};

export function objectStorage(): BlobStore {
  const client = new S3Client({
    endpoint: process.env.S3_ENDPOINT,
    bucket: process.env.S3_BUCKET,
    accessKeyId: process.env.S3_ACCESS_KEY_ID,
    secretAccessKey: process.env.S3_SECRET_ACCESS_KEY,
    region: "us-east-1",
  });
  return {
    async put(key, bytes) { await client.write(`v1/${key}`, bytes, { type: "application/json" }); },
    async get(key) { return client.file(`v1/${key}`).text(); },
  };
}
