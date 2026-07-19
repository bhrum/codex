//! Cloudflare Workers entrypoint and testable platform invariants.

#[cfg(any(target_arch = "wasm32", test))]
mod auth;

pub const PLATFORM_SCHEMA_V1: &str = include_str!("../migrations/0001_platform.sql");
pub const ACCOUNT_AUTH_SCHEMA_V2: &str =
    include_str!("../account-migrations/0001_account_auth.sql");

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    #[error("platform schema is missing required table {0}")]
    MissingTable(&'static str),
    #[error("platform schema must declare Rust-enforced posted journal balance")]
    MissingJournalBalanceInvariant,
    #[error("platform schema must declare Rust-enforced AI usage capacity")]
    MissingUsageCapacityInvariant,
    #[error("platform schema must use integer amounts")]
    FloatingPointAmount,
}

pub fn validate_platform_schema(schema: &str) -> Result<(), SchemaError> {
    for table in [
        "wallet_accounts",
        "journal_entries",
        "journal_lines",
        "products",
        "prices",
        "orders",
        "payment_attempts",
        "entitlements",
        "consumption_reservations",
        "refunds",
        "audit_events",
        "ai_usage_budgets",
        "ai_usage_reservations",
        "ai_usage_events",
    ] {
        let declaration = format!("CREATE TABLE IF NOT EXISTS {table}");
        if !schema.contains(&declaration) {
            return Err(SchemaError::MissingTable(table));
        }
    }
    if !schema.contains("journal_balance_enforced_by_worker_batch") {
        return Err(SchemaError::MissingJournalBalanceInvariant);
    }
    if !schema.contains("ai_usage_capacity_enforced_by_worker_batch") {
        return Err(SchemaError::MissingUsageCapacityInvariant);
    }
    if schema.lines().any(|line| {
        let line = line.trim().to_ascii_lowercase();
        line.contains("amount") && (line.contains(" real") || line.contains(" float"))
    }) {
        return Err(SchemaError::FloatingPointAmount);
    }
    Ok(())
}

pub fn validate_account_auth_schema(schema: &str) -> Result<(), SchemaError> {
    for table in [
        "account_password_credentials",
        "account_sessions",
        "account_refresh_tokens",
        "account_auth_events",
    ] {
        let declaration = format!("CREATE TABLE IF NOT EXISTS {table}");
        if !schema.contains(&declaration) {
            return Err(SchemaError::MissingTable(table));
        }
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
mod worker_api;

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
