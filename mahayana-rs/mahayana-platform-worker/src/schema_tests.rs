use super::*;
use pretty_assertions::assert_eq;

#[test]
fn migration_contains_the_authoritative_commerce_tables() {
    assert_eq!(validate_platform_schema(PLATFORM_SCHEMA_V1), Ok(()));
}

#[test]
fn account_auth_migration_contains_rotating_session_state() {
    assert_eq!(validate_account_auth_schema(ACCOUNT_AUTH_SCHEMA_V2), Ok(()));
}

#[test]
fn validation_rejects_floating_point_money() {
    let schema = PLATFORM_SCHEMA_V1.replace("amount INTEGER", "amount REAL");
    assert_eq!(
        validate_platform_schema(&schema),
        Err(SchemaError::FloatingPointAmount)
    );
}
