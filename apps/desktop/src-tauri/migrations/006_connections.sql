CREATE TABLE saved_connections (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES local_accounts(id),
    name TEXT NOT NULL,
    endpoint TEXT NOT NULL
);
CREATE INDEX saved_connections_account ON saved_connections (account_id);
