ALTER TABLE workspaces ADD COLUMN automation_cdp INTEGER NOT NULL DEFAULT 0 CHECK (automation_cdp IN (0,1));
CREATE TABLE automation_token (singleton INTEGER PRIMARY KEY CHECK (singleton=1), digest BLOB NOT NULL);
CREATE TABLE automation_audit (
  id INTEGER PRIMARY KEY,
  created INTEGER NOT NULL,
  account_id TEXT NOT NULL,
  operation TEXT NOT NULL,
  workspace_id TEXT,
  outcome TEXT NOT NULL
);
