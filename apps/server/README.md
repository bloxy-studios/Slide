# Server

The server publishes its OpenAPI contract at `GET /v1/openapi.json`. See the [Milestone E runbook](../../docs/milestone-e.md) for API token management, scope and rate limits, and sandbox limitations.

This is the Milestone D loopback development API. From the repository root:

```sh
docker compose up
```

The desktop **Shared** tab uses `http://127.0.0.1:4321/v1`. PostgreSQL and MinIO are private to the Compose network. The sample credentials in Compose are for local testing only. Do not expose this stack publicly.

For a native Bun run, configure `DATABASE_URL`, `S3_ENDPOINT`, `S3_BUCKET`, `S3_ACCESS_KEY_ID` and `S3_SECRET_ACCESS_KEY`, precreate the private bucket, and run `bun apps/server/index.ts`. The server applies the initial schema at startup. Use an isolated database and bucket for tests:

```sh
SLIDE_TEST_DATABASE_URL=<local-test-database-url> bun test apps/server
```

Supply the same S3 variables to exercise object storage; otherwise integration tests use an in-memory blob adapter. Without a test database URL these integration tests are skipped. The tests insert synthetic data and do not clean the append-only audit table.

See [the Milestone D runbook](../../docs/milestone-d.md) for permissions, routes and limits.
