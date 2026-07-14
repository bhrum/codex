use fabushi_telegram_media::MediaFileId;
use fabushi_telegram_media::TransferCommand;
use fabushi_telegram_media::TransferDirection;
use fabushi_telegram_media::TransferError;
use fabushi_telegram_media::TransferId;
use fabushi_telegram_media::TransferPriority;
use fabushi_telegram_media::TransferQueue;
use fabushi_telegram_media::TransferState;

fn enqueue(
    queue: &mut TransferQueue,
    id: &str,
    priority: TransferPriority,
    total_bytes: u64,
    chunk_size: u32,
    expected_sha256: Option<String>,
) {
    queue
        .execute(TransferCommand::Enqueue {
            id: TransferId::new(id),
            file_id: MediaFileId::new(format!("file-{id}")),
            direction: TransferDirection::Download,
            priority,
            total_bytes,
            chunk_size,
            expected_sha256,
        })
        .unwrap();
}

#[test]
fn scheduler_uses_priority_fifo_and_enforces_concurrency() {
    let mut queue = TransferQueue::new(1);
    enqueue(&mut queue, "normal", TransferPriority::Normal, 10, 4, None);
    enqueue(
        &mut queue,
        "realtime-first",
        TransferPriority::Realtime,
        10,
        4,
        None,
    );
    enqueue(
        &mut queue,
        "realtime-second",
        TransferPriority::Realtime,
        10,
        4,
        None,
    );
    assert_eq!(queue.next_ready().unwrap().id.0, "realtime-first");
    queue
        .execute(TransferCommand::Start {
            id: TransferId::new("realtime-first"),
        })
        .unwrap();
    assert!(queue.next_ready().is_none());
    assert_eq!(
        queue
            .execute(TransferCommand::Start {
                id: TransferId::new("normal"),
            })
            .unwrap_err(),
        TransferError::NoAvailableSlot
    );
}

#[test]
fn chunk_progress_pauses_and_resumes_from_exact_offset() {
    let mut queue = TransferQueue::new(2);
    enqueue(&mut queue, "resume", TransferPriority::Normal, 10, 4, None);
    let id = TransferId::new("resume");
    queue
        .execute(TransferCommand::Start { id: id.clone() })
        .unwrap();
    assert_eq!(queue.transfer(&id).unwrap().next_chunk().unwrap().length, 4);
    queue
        .execute(TransferCommand::RecordChunk {
            id: id.clone(),
            offset: 0,
            bytes_written: 4,
        })
        .unwrap();
    assert_eq!(queue.transfer(&id).unwrap().next_chunk().unwrap().offset, 4);
    assert_eq!(
        queue
            .execute(TransferCommand::RecordChunk {
                id: id.clone(),
                offset: 0,
                bytes_written: 4,
            })
            .unwrap_err(),
        TransferError::UnexpectedChunkOffset {
            expected: 4,
            actual: 0
        }
    );
    queue
        .execute(TransferCommand::Pause { id: id.clone() })
        .unwrap();
    queue
        .execute(TransferCommand::Resume { id: id.clone() })
        .unwrap();
    assert_eq!(queue.transfer(&id).unwrap().next_chunk().unwrap().offset, 4);
}

#[test]
fn completion_requires_all_bytes_and_matching_integrity_hash() {
    let expected = "ab".repeat(32);
    let mut queue = TransferQueue::new(1);
    enqueue(
        &mut queue,
        "hash",
        TransferPriority::UserInitiated,
        5,
        5,
        Some(expected.clone()),
    );
    let id = TransferId::new("hash");
    queue
        .execute(TransferCommand::Start { id: id.clone() })
        .unwrap();
    assert_eq!(
        queue
            .execute(TransferCommand::Complete {
                id: id.clone(),
                actual_sha256: Some(expected.clone()),
            })
            .unwrap_err(),
        TransferError::IncompleteTransfer {
            completed: 0,
            total: 5
        }
    );
    queue
        .execute(TransferCommand::RecordChunk {
            id: id.clone(),
            offset: 0,
            bytes_written: 5,
        })
        .unwrap();
    assert!(matches!(
        queue.execute(TransferCommand::Complete {
            id: id.clone(),
            actual_sha256: Some("cd".repeat(32)),
        }),
        Err(TransferError::IntegrityMismatch { .. })
    ));
    queue
        .execute(TransferCommand::Complete {
            id: id.clone(),
            actual_sha256: Some(expected.clone()),
        })
        .unwrap();
    assert_eq!(
        queue.transfer(&id).unwrap().state,
        TransferState::Completed {
            sha256: Some(expected)
        }
    );
}

#[test]
fn retryable_failure_returns_to_queue_without_losing_progress() {
    let mut queue = TransferQueue::new(1);
    enqueue(&mut queue, "retry", TransferPriority::Normal, 8, 4, None);
    let id = TransferId::new("retry");
    queue
        .execute(TransferCommand::Start { id: id.clone() })
        .unwrap();
    queue
        .execute(TransferCommand::RecordChunk {
            id: id.clone(),
            offset: 0,
            bytes_written: 4,
        })
        .unwrap();
    queue
        .execute(TransferCommand::Fail {
            id: id.clone(),
            code: "timeout".to_string(),
            retryable: true,
        })
        .unwrap();
    queue
        .execute(TransferCommand::Retry { id: id.clone() })
        .unwrap();
    let transfer = queue.transfer(&id).unwrap();
    assert_eq!(transfer.state, TransferState::Queued);
    assert_eq!(transfer.completed_bytes, 4);
    assert_eq!(transfer.attempt, 1);
}
