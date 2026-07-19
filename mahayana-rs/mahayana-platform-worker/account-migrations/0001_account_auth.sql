-- Rust account issuer state. Apply this migration to the same D1 database as
-- the existing users table and bind that database as ACCOUNT_DB.
--
-- Existing PBKDF2 columns are retained as read-only migration input. Successful
-- Rust logins write Argon2id credentials to the sidecar table; Rust never
-- rewrites the legacy columns and never issues a legacy token.

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS account_password_credentials (
    user_id TEXT PRIMARY KEY,
    password_phc TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS account_sessions (
    session_id TEXT PRIMARY KEY,
    refresh_family_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    current_refresh_token_hash TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    revoked_reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_account_sessions_user
    ON account_sessions(user_id, revoked_at, expires_at);
CREATE INDEX IF NOT EXISTS idx_account_sessions_family
    ON account_sessions(refresh_family_id);

CREATE TABLE IF NOT EXISTS account_refresh_tokens (
    token_hash TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('active', 'used', 'revoked')),
    issued_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at INTEGER,
    replaced_by_hash TEXT,
    UNIQUE(session_id, generation),
    FOREIGN KEY (session_id) REFERENCES account_sessions(session_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_account_refresh_session
    ON account_refresh_tokens(session_id, state);

CREATE TABLE IF NOT EXISTS account_auth_events (
    event_id TEXT PRIMARY KEY,
    user_id TEXT,
    session_id TEXT,
    event_type TEXT NOT NULL,
    occurred_at INTEGER NOT NULL,
    details_json TEXT
);

CREATE INDEX IF NOT EXISTS idx_account_auth_events_user
    ON account_auth_events(user_id, occurred_at);
