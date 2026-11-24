//! One-time recovery codes, stored only as SHA-256 hashes.

use sha2::{Digest, Sha256};
use webx_core::security::constant_time_eq;

/// Default number of recovery codes issued per user.
pub const DEFAULT_RECOVERY_CODE_COUNT: usize = 10;
/// Characters per recovery code (excluding the display hyphen).
pub const RECOVERY_CODE_LEN: usize = 10;

/// Persisted state for one recovery code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryCodeRecord {
    /// SHA-256 of the normalized code, lowercase hex.
    pub hash: String,
    /// Whether the code was already consumed.
    pub used: bool,
}

/// The hashed recovery codes of one user.
///
/// [`RecoveryCodeSet::verify_and_consume`] gives atomic single-use semantics
/// for a single in-memory set. A database-backed deployment persists
/// [`RecoveryCodeRecord`]s and performs the equivalent conditional update.
/// [`RecoveryCodeSet::find`] / [`RecoveryCodeSet::consume_matched`] return the
/// matched record so the adapter can use its [`RecoveryCodeRecord::hash`] in one
/// statement, e.g.
/// `UPDATE mfa_recovery_codes SET used_at = ? WHERE user_id = ? AND code_hash = ? AND used_at IS NULL`
/// (0 rows affected ⇒ the code was already consumed). The hash format here is
/// the contract.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryCodeSet {
    records: Vec<RecoveryCodeRecord>,
}

impl RecoveryCodeSet {
    /// Generate `count` codes; returns the plaintext (show once) and hashes.
    pub fn generate(count: usize) -> (Vec<String>, Self) {
        let mut plaintext = Vec::with_capacity(count);
        let mut records = Vec::with_capacity(count);
        for _ in 0..count {
            let code = generate_code();
            records.push(RecoveryCodeRecord {
                hash: hash_code(&code),
                used: false,
            });
            plaintext.push(code);
        }
        (plaintext, Self { records })
    }

    /// Rebuild from persisted records.
    pub fn from_records(records: Vec<RecoveryCodeRecord>) -> Self {
        Self { records }
    }

    /// Stored records (for persistence).
    pub fn records(&self) -> &[RecoveryCodeRecord] {
        &self.records
    }

    /// Unused codes remaining.
    pub fn remaining(&self) -> usize {
        self.records.iter().filter(|record| !record.used).count()
    }

    /// Find the record matching `candidate` without consuming it.
    ///
    /// Accepts the display form (`ABCDE-FGHIJ`), lowercase, and surrounding
    /// whitespace. The returned record's [`RecoveryCodeRecord::hash`] is the
    /// value a database adapter should use for its single conditional update
    /// (`WHERE code_hash = ? AND used = 0`), so the candidate is normalized and
    /// hashed exactly once. Already-consumed records are still returned (with
    /// [`RecoveryCodeRecord::used`] set), letting the caller tell a replayed
    /// code from a wrong one.
    ///
    /// This does not consume: for the in-memory single-use guarantee use
    /// [`RecoveryCodeSet::consume_matched`] (or `verify_and_consume`).
    pub fn find(&self, candidate: &str) -> Option<&RecoveryCodeRecord> {
        let index = self.find_index(candidate)?;
        self.records.get(index)
    }

    /// Consume and return the record matching `candidate`, if it is unused.
    ///
    /// This is the in-memory equivalent of the database CAS described on
    /// [`RecoveryCodeSet`]: the returned record is the pre-consumption value
    /// (its `hash` addresses the persisted row) and this set's copy is marked
    /// [`RecoveryCodeRecord::used`], so a second call with the same candidate
    /// returns `None`.
    pub fn consume_matched(&mut self, candidate: &str) -> Option<RecoveryCodeRecord> {
        let index = self.find_index(candidate)?;
        let record = self.records.get_mut(index)?;
        if record.used {
            return None;
        }
        let matched = record.clone();
        record.used = true;
        Some(matched)
    }

    /// Verify and consume one code.
    ///
    /// Accepts the display form (`ABCDE-FGHIJ`), lowercase, and surrounding
    /// whitespace. A consumed code is never accepted twice. Thin boolean
    /// wrapper over [`RecoveryCodeSet::consume_matched`] for callers that need
    /// only the verdict.
    pub fn verify_and_consume(&mut self, candidate: &str) -> bool {
        self.consume_matched(candidate).is_some()
    }

    fn find_index(&self, candidate: &str) -> Option<usize> {
        let normalized = normalize_code(candidate)?;
        let hash = hash_code(&normalized);
        let mut matched = None;
        for (index, record) in self.records.iter().enumerate() {
            if constant_time_eq(record.hash.as_bytes(), hash.as_bytes()) {
                matched = Some(index);
            }
        }
        matched
    }
}

/// SHA-256 of the normalized code, lowercase hex.
///
/// Accepts the display form when the code has the expected length; arbitrary
/// strings are uppercased as-is so callers cannot accidentally hash the
/// hyphenated form of a real code.
pub fn hash_code(code: &str) -> String {
    let normalized = normalize_code(code).unwrap_or_else(|| code.to_ascii_uppercase());
    let digest = Sha256::digest(normalized.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn normalize_code(code: &str) -> Option<String> {
    let normalized: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (normalized.len() == RECOVERY_CODE_LEN).then_some(normalized)
}

fn generate_code() -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    let mut code = String::with_capacity(RECOVERY_CODE_LEN + 1);
    for (index, byte) in bytes.iter().take(RECOVERY_CODE_LEN).enumerate() {
        if index == 5 {
            code.push('-');
        }
        code.push(ALPHABET[(byte & 0x1f) as usize] as char);
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_codes_are_unique_hashed_and_single_use() {
        let (codes, mut set) = RecoveryCodeSet::generate(5);
        assert_eq!(codes.len(), 5);
        assert_eq!(set.remaining(), 5);
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), 5);
        for record in set.records() {
            assert!(!codes.contains(&record.hash));
            assert_eq!(record.hash.len(), 64);
        }

        assert!(set.verify_and_consume(&codes[0]));
        assert_eq!(set.remaining(), 4);
        assert!(
            !set.verify_and_consume(&codes[0]),
            "a recovery code is single-use"
        );
    }

    #[test]
    fn display_format_case_and_whitespace_are_normalized() {
        let (codes, mut set) = RecoveryCodeSet::generate(1);
        let display = codes[0].to_ascii_lowercase();
        assert!(set.verify_and_consume(&format!("  {display}  ")));
    }

    #[test]
    fn wrong_code_does_not_consume_another() {
        let (codes, mut set) = RecoveryCodeSet::generate(2);
        assert!(!set.verify_and_consume("AAAAA-AAAAA"));
        assert_eq!(set.remaining(), 2);
        assert!(set.verify_and_consume(&codes[1]));
    }

    #[test]
    fn find_returns_the_matching_record_hash_without_consuming() {
        let (codes, set) = RecoveryCodeSet::generate(3);
        let lowered = codes[1].to_ascii_lowercase();
        let record = set
            .find(&format!("  {lowered}  "))
            .expect("the code matches");
        assert_eq!(record.hash, hash_code(&codes[1]));
        assert!(!record.used);
        assert_eq!(set.remaining(), 3, "find must not consume");
        assert!(set.find("AAAAA-AAAAA").is_none());
        assert!(set.find("too-short").is_none());
    }

    #[test]
    fn consume_matched_returns_the_record_once() {
        let (codes, mut set) = RecoveryCodeSet::generate(2);
        let consumed = set.consume_matched(&codes[0]).expect("first use");
        assert_eq!(consumed.hash, hash_code(&codes[0]));
        assert!(!consumed.used, "the clone is the pre-consumption value");
        assert_eq!(set.remaining(), 1);

        assert!(set.consume_matched(&codes[0]).is_none(), "single use");
        assert!(!set.verify_and_consume(&codes[0]), "single use via wrapper");
        assert_eq!(set.remaining(), 1);
        assert!(
            set.find(&codes[0])
                .expect("used record stays findable")
                .used,
            "find reports the consumed record for the CAS path"
        );
    }

    #[test]
    fn persisted_used_records_are_not_consumable() {
        let records = vec![
            RecoveryCodeRecord {
                hash: hash_code("AAAAA-AAAAA"),
                used: true,
            },
            RecoveryCodeRecord {
                hash: hash_code("BBBBB-BBBBB"),
                used: false,
            },
        ];
        let mut set = RecoveryCodeSet::from_records(records);
        assert!(set.find("AAAAA-AAAAA").is_some());
        assert!(set.consume_matched("AAAAA-AAAAA").is_none());
        assert!(!set.verify_and_consume("AAAAA-AAAAA"));
        assert_eq!(set.remaining(), 1);
        assert!(set.consume_matched("BBBBB-BBBBB").is_some());
        assert_eq!(set.remaining(), 0);
    }

    #[test]
    fn persisted_records_round_trip() {
        let (codes, set) = RecoveryCodeSet::generate(1);
        let rebuilt = RecoveryCodeSet::from_records(set.records().to_vec());
        assert_eq!(rebuilt.records(), set.records());
        assert_eq!(rebuilt.remaining(), 1);
        assert!(codes[0].contains('-'));
    }
}
