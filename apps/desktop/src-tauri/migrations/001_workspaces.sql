CREATE TABLE workspaces (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    start_url TEXT,
    pid INTEGER,
    started INTEGER,
    exit_note TEXT
);

CREATE TABLE runtime_settings (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    path TEXT NOT NULL
);
