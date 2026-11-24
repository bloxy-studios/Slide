CREATE TABLE workspace_crx (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    catalog_id TEXT NOT NULL,
    browser_id TEXT NOT NULL,
    version TEXT NOT NULL,
    package_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, catalog_id),
    UNIQUE (workspace_id, browser_id)
);
