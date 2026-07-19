use super::*;
use pretty_assertions::assert_eq;

#[test]
fn balanced_entry_is_accepted() {
    let entry = JournalEntry {
        entry_id: "entry-1".into(),
        reference_type: "purchase".into(),
        reference_id: "order-1".into(),
        created_at: 1,
        lines: vec![
            JournalLine {
                account_id: "user-liability".into(),
                currency: Currency("MAHAYANA_CREDIT".into()),
                amount: -100,
            },
            JournalLine {
                account_id: "platform-revenue".into(),
                currency: Currency("MAHAYANA_CREDIT".into()),
                amount: 100,
            },
        ],
    };

    assert_eq!(entry.validate(), Ok(()));
}

#[test]
fn unbalanced_entry_is_rejected_per_currency() {
    let entry = JournalEntry {
        entry_id: "entry-1".into(),
        reference_type: "purchase".into(),
        reference_id: "order-1".into(),
        created_at: 1,
        lines: vec![
            JournalLine {
                account_id: "user-liability".into(),
                currency: Currency("MAHAYANA_CREDIT".into()),
                amount: -100,
            },
            JournalLine {
                account_id: "platform-revenue".into(),
                currency: Currency("MAHAYANA_CREDIT".into()),
                amount: 99,
            },
        ],
    };

    assert_eq!(
        entry.validate(),
        Err(LedgerError::Unbalanced {
            currency: "MAHAYANA_CREDIT".into(),
            amount: -1,
        })
    );
}
