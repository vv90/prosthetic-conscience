use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::consensus::seed::{SeedParseError, load_entries_from_value};
use crate::consensus::types::Entry;

use super::session::{SessionClient, SessionError, SessionEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedResult {
    pub session_id: String,
    pub total_entries: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    #[error("failed to read fixture file {path}: {source}")]
    ReadFixture {
        path: String,
        source: std::io::Error,
    },
    #[error("{0}")]
    Parse(#[from] SeedParseError),
    #[error("failed to serialize entry for append: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("session error: {0}")]
    Session(#[from] SessionError),
    #[error("gateway echoed entry #{actual}, expected #{expected}")]
    OutOfOrderEcho { expected: usize, actual: usize },
    #[error("gateway echoed unexpected payload at entry #{index}")]
    EchoPayloadMismatch {
        index: usize,
        expected: Value,
        actual: Value,
    },
    #[error("gateway persisted unexpected payload at entry #{index}")]
    PersistedPayloadMismatch {
        index: usize,
        expected: Value,
        actual: Value,
    },
    #[error("gateway persisted {actual} entries, expected {expected}")]
    PersistedLengthMismatch { expected: usize, actual: usize },
}

pub fn load_entries_from_path(path: &Path) -> Result<Vec<Entry>, SeedError> {
    let text = fs::read_to_string(path).map_err(|source| SeedError::ReadFixture {
        path: path.display().to_string(),
        source,
    })?;
    let value: Value =
        serde_json::from_str(&text).map_err(|e| SeedParseError::InvalidFixture(e.to_string()))?;
    Ok(load_entries_from_value(value)?)
}

pub async fn join_and_seed_session(
    base_url: String,
    auth_token: Option<String>,
    session_id: String,
    entries: &[Entry],
) -> Result<SeedResult, SeedError> {
    let mut session = SessionClient::join(base_url, auth_token, session_id).await?;
    seed_session(&mut session, entries).await?;
    Ok(SeedResult {
        session_id: session.session_id().to_owned(),
        total_entries: entries.len(),
    })
}

pub async fn seed_session(session: &mut SessionClient, entries: &[Entry]) -> Result<(), SeedError> {
    let payloads = entries
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()?;

    let mut next_index = 0usize;
    while next_index < payloads.len() {
        let waiting_for = next_index;
        let payload = payloads[waiting_for].clone();

        match session.append_json(payload).await {
            Ok(()) => {
                wait_for_expected_echo(session, &payloads, &mut next_index).await?;
            }
            Err(SessionError::Disconnected(_)) => {
                wait_for_reconnect_and_sync(session, &payloads, &mut next_index).await?;
            }
            Err(error) => return Err(error.into()),
        }

        if next_index <= waiting_for {
            continue;
        }
    }

    verify_persisted_entries(session, &payloads).await
}

async fn wait_for_expected_echo(
    session: &mut SessionClient,
    payloads: &[Value],
    next_index: &mut usize,
) -> Result<(), SeedError> {
    loop {
        match session.next_event().await? {
            SessionEvent::Entry { index, payload } => {
                if index < *next_index {
                    continue;
                }
                if index > *next_index {
                    return Err(SeedError::OutOfOrderEcho {
                        expected: *next_index,
                        actual: index,
                    });
                }

                let expected = &payloads[*next_index];
                if payload != *expected {
                    return Err(SeedError::EchoPayloadMismatch {
                        index,
                        expected: expected.clone(),
                        actual: payload,
                    });
                }

                *next_index += 1;
                return Ok(());
            }
            SessionEvent::Disconnected { .. } => {
                wait_for_reconnect_and_sync(session, payloads, next_index).await?;
                return Ok(());
            }
            SessionEvent::Reconnected => {
                sync_persisted_entries(session, payloads, next_index).await?;
                return Ok(());
            }
            SessionEvent::Warning(_) => {}
        }
    }
}

async fn wait_for_reconnect_and_sync(
    session: &mut SessionClient,
    payloads: &[Value],
    next_index: &mut usize,
) -> Result<(), SeedError> {
    loop {
        match session.next_event().await? {
            SessionEvent::Reconnected => {
                sync_persisted_entries(session, payloads, next_index).await?;
                return Ok(());
            }
            SessionEvent::Entry { index, payload } => {
                if index < *next_index {
                    continue;
                }
                if index > *next_index {
                    return Err(SeedError::OutOfOrderEcho {
                        expected: *next_index,
                        actual: index,
                    });
                }

                let expected = &payloads[*next_index];
                if payload != *expected {
                    return Err(SeedError::EchoPayloadMismatch {
                        index,
                        expected: expected.clone(),
                        actual: payload,
                    });
                }

                *next_index += 1;
                return Ok(());
            }
            SessionEvent::Disconnected { .. } | SessionEvent::Warning(_) => {}
        }
    }
}

async fn sync_persisted_entries(
    session: &SessionClient,
    payloads: &[Value],
    next_index: &mut usize,
) -> Result<(), SeedError> {
    if *next_index >= payloads.len() {
        return Ok(());
    }

    let remaining = payloads.len() - *next_index;
    let page = session.fetch_entries(*next_index, remaining).await?;
    let persisted = page.entries.len();
    for (offset, actual) in page.entries.into_iter().enumerate() {
        let index = *next_index + offset;
        let expected = &payloads[index];
        if actual != *expected {
            return Err(SeedError::PersistedPayloadMismatch {
                index,
                expected: expected.clone(),
                actual,
            });
        }
    }

    *next_index += persisted;
    Ok(())
}

async fn verify_persisted_entries(
    session: &SessionClient,
    payloads: &[Value],
) -> Result<(), SeedError> {
    let page = session.fetch_entries(0, payloads.len()).await?;
    if page.total != payloads.len() || page.entries.len() != payloads.len() {
        return Err(SeedError::PersistedLengthMismatch {
            expected: payloads.len(),
            actual: page.total,
        });
    }

    for (index, (expected, actual)) in payloads.iter().zip(page.entries.into_iter()).enumerate() {
        if actual != *expected {
            return Err(SeedError::PersistedPayloadMismatch {
                index,
                expected: expected.clone(),
                actual,
            });
        }
    }

    Ok(())
}
