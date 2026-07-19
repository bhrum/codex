use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Currency(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub quote_id: String,
    pub plugin_id: String,
    pub sku: String,
    pub amount: i64,
    pub currency: Currency,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PurchaseRequest {
    pub sku: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntitlementStatus {
    Active,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entitlement {
    pub entitlement_id: String,
    pub user_id: String,
    pub plugin_id: String,
    pub capability: String,
    pub status: EntitlementStatus,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalLine {
    pub account_id: String,
    pub currency: Currency,
    pub amount: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    pub entry_id: String,
    pub reference_type: String,
    pub reference_id: String,
    pub created_at: i64,
    pub lines: Vec<JournalLine>,
}

impl JournalEntry {
    pub fn validate(&self) -> Result<(), LedgerError> {
        if self.lines.len() < 2 {
            return Err(LedgerError::NotEnoughLines);
        }
        let mut totals = BTreeMap::<&Currency, i128>::new();
        for line in &self.lines {
            if line.amount == 0 {
                return Err(LedgerError::ZeroLine);
            }
            *totals.entry(&line.currency).or_default() += i128::from(line.amount);
        }
        if let Some((currency, amount)) = totals.into_iter().find(|(_, amount)| *amount != 0) {
            return Err(LedgerError::Unbalanced {
                currency: currency.0.clone(),
                amount,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LedgerError {
    #[error("journal entries require at least two lines")]
    NotEnoughLines,
    #[error("journal lines must not have zero amount")]
    ZeroLine,
    #[error("journal entry is unbalanced for {currency}: {amount}")]
    Unbalanced { currency: String, amount: i128 },
}

#[cfg(test)]
#[path = "commerce_tests.rs"]
mod tests;
