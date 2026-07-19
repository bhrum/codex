PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS marketplace_plugins (
    plugin_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    description TEXT NOT NULL,
    publisher_user_id TEXT NOT NULL,
    latest_version TEXT,
    visibility TEXT NOT NULL CHECK (visibility IN ('public', 'unlisted', 'private')),
    review_state TEXT NOT NULL CHECK (review_state IN ('draft', 'pending', 'approved', 'rejected')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS plugin_releases (
    plugin_id TEXT NOT NULL REFERENCES marketplace_plugins(plugin_id) ON DELETE RESTRICT,
    version TEXT NOT NULL,
    package_key TEXT NOT NULL UNIQUE,
    package_sha256 TEXT NOT NULL,
    package_size INTEGER NOT NULL CHECK (package_size > 0),
    tuf_target_path TEXT NOT NULL UNIQUE,
    published_at INTEGER NOT NULL,
    PRIMARY KEY (plugin_id, version)
);

CREATE TABLE IF NOT EXISTS wallet_accounts (
    account_id TEXT PRIMARY KEY,
    owner_type TEXT NOT NULL CHECK (owner_type IN ('user', 'platform', 'developer')),
    owner_id TEXT NOT NULL,
    currency TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (owner_type, owner_id, currency)
);

CREATE TABLE IF NOT EXISTS journal_entries (
    entry_id TEXT PRIMARY KEY,
    reference_type TEXT NOT NULL,
    reference_id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'draft' CHECK (state IN ('draft', 'posted', 'void')),
    created_at INTEGER NOT NULL,
    posted_at INTEGER,
    UNIQUE (reference_type, reference_id)
);

CREATE TABLE IF NOT EXISTS journal_lines (
    line_id TEXT PRIMARY KEY,
    entry_id TEXT NOT NULL REFERENCES journal_entries(entry_id) ON DELETE RESTRICT,
    account_id TEXT NOT NULL REFERENCES wallet_accounts(account_id) ON DELETE RESTRICT,
    currency TEXT NOT NULL,
    amount INTEGER NOT NULL CHECK (amount <> 0),
    created_at INTEGER NOT NULL,
    UNIQUE (entry_id, account_id)
);

CREATE INDEX IF NOT EXISTS journal_lines_account_idx
    ON journal_lines(account_id, currency, created_at);

-- D1's production query endpoint does not accept CREATE TRIGGER bodies.
-- journal_balance_enforced_by_worker_batch: the Rust purchase transaction
-- posts an entry only when it has at least two lines and every currency sums
-- to zero. No public route can write journal tables directly.

CREATE TABLE IF NOT EXISTS products (
    product_id TEXT PRIMARY KEY,
    plugin_id TEXT NOT NULL,
    sku TEXT NOT NULL,
    seller_user_id TEXT,
    entitlement_capability TEXT NOT NULL,
    consumption_mode TEXT NOT NULL CHECK (consumption_mode IN ('durable', 'consumable')),
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (plugin_id, sku)
);

CREATE TABLE IF NOT EXISTS prices (
    price_id TEXT PRIMARY KEY,
    product_id TEXT NOT NULL REFERENCES products(product_id) ON DELETE RESTRICT,
    currency TEXT NOT NULL,
    amount INTEGER NOT NULL CHECK (amount > 0),
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    starts_at INTEGER NOT NULL,
    ends_at INTEGER,
    created_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS prices_one_active_idx
    ON prices(product_id, currency) WHERE active = 1;

CREATE TABLE IF NOT EXISTS orders (
    order_id TEXT PRIMARY KEY,
    buyer_user_id TEXT NOT NULL,
    plugin_id TEXT NOT NULL,
    product_id TEXT NOT NULL REFERENCES products(product_id) ON DELETE RESTRICT,
    price_id TEXT NOT NULL REFERENCES prices(price_id) ON DELETE RESTRICT,
    sku TEXT NOT NULL,
    currency TEXT NOT NULL,
    amount INTEGER NOT NULL CHECK (amount > 0),
    status TEXT NOT NULL CHECK (status IN ('pending', 'paid', 'fulfilled', 'failed', 'refunded')),
    idempotency_key TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (buyer_user_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS payment_attempts (
    attempt_id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES orders(order_id) ON DELETE RESTRICT,
    provider TEXT NOT NULL,
    provider_event_id TEXT,
    provider_payment_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('created', 'authorized', 'captured', 'failed', 'cancelled', 'refunded')),
    request_fingerprint TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (provider, provider_event_id)
);

CREATE TABLE IF NOT EXISTS entitlements (
    entitlement_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    plugin_id TEXT NOT NULL,
    product_id TEXT NOT NULL REFERENCES products(product_id) ON DELETE RESTRICT,
    order_id TEXT NOT NULL REFERENCES orders(order_id) ON DELETE RESTRICT,
    capability TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked', 'expired')),
    granted_at INTEGER NOT NULL,
    expires_at INTEGER,
    revoked_at INTEGER,
    UNIQUE (order_id, capability)
);

CREATE INDEX IF NOT EXISTS entitlements_lookup_idx
    ON entitlements(user_id, plugin_id, capability, status);

CREATE TABLE IF NOT EXISTS consumption_reservations (
    reservation_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    plugin_id TEXT NOT NULL,
    product_id TEXT NOT NULL REFERENCES products(product_id) ON DELETE RESTRICT,
    entitlement_id TEXT REFERENCES entitlements(entitlement_id) ON DELETE RESTRICT,
    currency TEXT NOT NULL,
    amount INTEGER NOT NULL CHECK (amount > 0),
    state TEXT NOT NULL CHECK (state IN ('reserved', 'captured', 'released', 'expired')),
    idempotency_key TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (user_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS refunds (
    refund_id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES orders(order_id) ON DELETE RESTRICT,
    provider_refund_id TEXT,
    currency TEXT NOT NULL,
    amount INTEGER NOT NULL CHECK (amount > 0),
    reason TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('requested', 'processing', 'succeeded', 'failed')),
    idempotency_key TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS audit_events (
    event_id TEXT PRIMARY KEY,
    actor_type TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    subject_type TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS audit_events_subject_idx
    ON audit_events(subject_type, subject_id, created_at);

-- Authoritative AI usage accounting. The model gateway reserves a bounded
-- request before inference and captures only provider-reported usage after
-- completion. Clients can read these rows but cannot write them directly.
CREATE TABLE IF NOT EXISTS ai_usage_budgets (
    user_id TEXT NOT NULL,
    window_start INTEGER NOT NULL,
    window_end INTEGER NOT NULL,
    token_limit INTEGER NOT NULL CHECK (token_limit >= 0),
    used_tokens INTEGER NOT NULL DEFAULT 0 CHECK (used_tokens >= 0),
    reserved_tokens INTEGER NOT NULL DEFAULT 0 CHECK (reserved_tokens >= 0),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, window_start),
    CHECK (window_end > window_start),
    CHECK (used_tokens + reserved_tokens <= token_limit)
);

CREATE TABLE IF NOT EXISTS ai_usage_reservations (
    reservation_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    window_start INTEGER NOT NULL,
    request_id TEXT NOT NULL,
    input_token_budget INTEGER NOT NULL CHECK (input_token_budget >= 0),
    output_token_budget INTEGER NOT NULL CHECK (output_token_budget >= 0),
    reserved_tokens INTEGER NOT NULL CHECK (reserved_tokens > 0),
    actual_input_tokens INTEGER CHECK (actual_input_tokens >= 0),
    actual_cached_input_tokens INTEGER CHECK (actual_cached_input_tokens >= 0),
    actual_output_tokens INTEGER CHECK (actual_output_tokens >= 0),
    actual_reasoning_output_tokens INTEGER CHECK (actual_reasoning_output_tokens >= 0),
    actual_total_tokens INTEGER CHECK (actual_total_tokens >= 0),
    state TEXT NOT NULL CHECK (state IN ('reserved', 'captured', 'released', 'expired')),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (user_id, request_id),
    FOREIGN KEY (user_id, window_start)
        REFERENCES ai_usage_budgets(user_id, window_start) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS ai_usage_reservations_expiry_idx
    ON ai_usage_reservations(state, expires_at);

CREATE TABLE IF NOT EXISTS ai_usage_events (
    event_id TEXT PRIMARY KEY,
    reservation_id TEXT NOT NULL REFERENCES ai_usage_reservations(reservation_id) ON DELETE RESTRICT,
    provider_response_id TEXT NOT NULL UNIQUE,
    input_tokens INTEGER NOT NULL CHECK (input_tokens >= 0),
    cached_input_tokens INTEGER NOT NULL CHECK (cached_input_tokens >= 0),
    output_tokens INTEGER NOT NULL CHECK (output_tokens >= 0),
    reasoning_output_tokens INTEGER NOT NULL CHECK (reasoning_output_tokens >= 0),
    total_tokens INTEGER NOT NULL CHECK (total_tokens >= 0),
    created_at INTEGER NOT NULL
);

-- ai_usage_capacity_enforced_by_worker_batch: Rust uses conditional INSERT,
-- event-gated capture and ordered D1 batch transactions so reservation and
-- budget state change atomically without database triggers.

CREATE VIEW IF NOT EXISTS wallet_balances AS
SELECT
    wa.account_id,
    wa.owner_type,
    wa.owner_id,
    wa.currency,
    COALESCE(SUM(CASE WHEN je.state = 'posted' THEN jl.amount ELSE 0 END), 0) AS balance
FROM wallet_accounts wa
LEFT JOIN journal_lines jl ON jl.account_id = wa.account_id AND jl.currency = wa.currency
LEFT JOIN journal_entries je ON je.entry_id = jl.entry_id
GROUP BY wa.account_id, wa.owner_type, wa.owner_id, wa.currency;
