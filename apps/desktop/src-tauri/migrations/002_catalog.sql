CREATE TABLE local_accounts (id TEXT PRIMARY KEY, name TEXT NOT NULL);
INSERT INTO local_accounts VALUES ('local', 'Personal');

CREATE TABLE current_account (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    id TEXT NOT NULL REFERENCES local_accounts(id)
);
INSERT INTO current_account VALUES (1, 'local');

CREATE TABLE account_options (
    account_id TEXT PRIMARY KEY REFERENCES local_accounts(id),
    default_template TEXT,
    view TEXT NOT NULL DEFAULT '{}'
);
INSERT INTO account_options (account_id) VALUES ('local');

ALTER TABLE workspaces ADD COLUMN account_id TEXT NOT NULL DEFAULT 'local';
ALTER TABLE workspaces ADD COLUMN settings TEXT NOT NULL DEFAULT '{}';
ALTER TABLE workspaces ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workspaces ADD COLUMN trashed_at INTEGER;
CREATE INDEX workspaces_account ON workspaces (account_id, trashed_at);

CREATE TABLE catalog_items (
    account_id TEXT NOT NULL REFERENCES local_accounts(id),
    kind TEXT NOT NULL CHECK (kind IN ('folder', 'tag', 'bookmark', 'template', 'extension')),
    id TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (account_id, kind, id)
);

CREATE TABLE tasks (
    account_id TEXT NOT NULL REFERENCES local_accounts(id),
    id TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    due_at INTEGER NOT NULL,
    fired_at INTEGER,
    completed_at INTEGER,
    PRIMARY KEY (account_id, id)
);
CREATE INDEX tasks_due ON tasks (account_id, completed_at, fired_at, due_at);

CREATE TABLE task_workspaces (
    account_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    PRIMARY KEY (account_id, task_id, workspace_id),
    FOREIGN KEY (account_id, task_id) REFERENCES tasks(account_id, id) ON DELETE CASCADE
);

CREATE TABLE completed_import_rows (
    account_id TEXT NOT NULL REFERENCES local_accounts(id),
    fingerprint TEXT NOT NULL,
    row_number INTEGER NOT NULL,
    workspace_id TEXT NOT NULL,
    PRIMARY KEY (account_id, fingerprint, row_number)
);
