CREATE TABLE IF NOT EXISTS accounts (
  id uuid PRIMARY KEY,
  email text UNIQUE NOT NULL,
  password_hash text NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
  digest text PRIMARY KEY,
  account_id uuid NOT NULL REFERENCES accounts(id),
  device_id uuid NOT NULL,
  expires_at timestamptz NOT NULL
);
CREATE TABLE IF NOT EXISTS teams (
  id uuid PRIMARY KEY,
  name text NOT NULL
);
CREATE TABLE IF NOT EXISTS members (
  team_id uuid REFERENCES teams(id),
  account_id uuid REFERENCES accounts(id),
  role text NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
  PRIMARY KEY (team_id, account_id)
);
CREATE TABLE IF NOT EXISTS invitations (
  id uuid PRIMARY KEY,
  team_id uuid NOT NULL REFERENCES teams(id),
  email text NOT NULL,
  role text NOT NULL CHECK (role IN ('admin', 'member')),
  expires_at timestamptz NOT NULL
);
CREATE TABLE IF NOT EXISTS workspaces (
  id uuid PRIMARY KEY,
  team_id uuid NOT NULL REFERENCES teams(id),
  owner_id uuid NOT NULL REFERENCES accounts(id),
  name text NOT NULL,
  generation integer NOT NULL DEFAULT 0,
  fence bigint NOT NULL DEFAULT 0,
  holder_id uuid REFERENCES accounts(id),
  holder_device uuid,
  lease_until timestamptz
);
CREATE INDEX IF NOT EXISTS workspaces_team ON workspaces(team_id);
CREATE TABLE IF NOT EXISTS grants (
  workspace_id uuid REFERENCES workspaces(id),
  account_id uuid REFERENCES accounts(id),
  role text NOT NULL CHECK (role IN ('reader', 'operator')),
  PRIMARY KEY (workspace_id, account_id)
);
CREATE TABLE IF NOT EXISTS revisions (
  workspace_id uuid REFERENCES workspaces(id),
  version integer NOT NULL,
  request_id uuid NOT NULL,
  actor_id uuid NOT NULL REFERENCES accounts(id),
  blob_key uuid NOT NULL,
  sha256 text NOT NULL,
  size integer NOT NULL,
  restored_from integer,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (workspace_id, version),
  UNIQUE (workspace_id, request_id)
);
CREATE TABLE IF NOT EXISTS exchange_tickets (
  digest text PRIMARY KEY,
  workspace_id uuid NOT NULL,
  version integer NOT NULL,
  recipient_id uuid NOT NULL REFERENCES accounts(id),
  expires_at timestamptz NOT NULL,
  redeemed_at timestamptz,
  FOREIGN KEY (workspace_id, version) REFERENCES revisions(workspace_id, version)
);
CREATE TABLE IF NOT EXISTS audit_events (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  team_id uuid NOT NULL REFERENCES teams(id),
  actor_id uuid NOT NULL REFERENCES accounts(id),
  subject_id uuid NOT NULL,
  action text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX IF NOT EXISTS audit_events_team ON audit_events(team_id, id);
CREATE OR REPLACE FUNCTION slide_audit_immutable() RETURNS trigger
  LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'audit events are append-only'; END; $$;
DROP TRIGGER IF EXISTS audit_immutable ON audit_events;
CREATE TRIGGER audit_immutable BEFORE UPDATE OR DELETE OR TRUNCATE ON audit_events
  FOR EACH STATEMENT EXECUTE FUNCTION slide_audit_immutable();
