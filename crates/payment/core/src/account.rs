//! Multi-account credential routing shared by all adapters.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::error::PaymentError;
use crate::ids::{AccountId, ProviderId};
use crate::model::PaymentAccount;

/// Resolved credentials for one merchant account.
///
/// Adapters store their provider-specific credential struct in `T`; the set
/// never exposes secrets through [`AccountSet::accounts`] (only the public
/// [`PaymentAccount`] view).
#[derive(Clone)]
pub struct AccountSet<T> {
    provider: ProviderId,
    default: AccountId,
    accounts: BTreeMap<AccountId, Arc<T>>,
    public: Vec<PaymentAccount>,
}

impl<T> AccountSet<T> {
    /// Build a set and validate it eagerly.
    ///
    /// Fails when the set is empty, contains duplicate ids, or the default id
    /// is not present.
    pub fn new(
        provider: ProviderId,
        default: AccountId,
        accounts: impl IntoIterator<Item = (AccountId, T, PaymentAccount)>,
    ) -> Result<Self, PaymentError> {
        let mut by_id = BTreeMap::new();
        let mut public = Vec::new();
        for (id, value, info) in accounts {
            if by_id.contains_key(&id) {
                return Err(PaymentError::config(
                    provider.clone(),
                    format!("duplicate account id `{id}`"),
                ));
            }
            by_id.insert(id.clone(), Arc::new(value));
            public.push(info);
        }
        if by_id.is_empty() {
            return Err(PaymentError::config(
                provider.clone(),
                "at least one account must be configured",
            ));
        }
        if !by_id.contains_key(&default) {
            return Err(PaymentError::config(
                provider.clone(),
                format!("default account `{default}` is not configured"),
            ));
        }
        Ok(Self {
            provider,
            default,
            accounts: by_id,
            public,
        })
    }

    /// Build a single-account set from the default account id.
    pub fn single(
        provider: ProviderId,
        value: T,
        mut info: PaymentAccount,
    ) -> Result<Self, PaymentError> {
        let default = AccountId::default_account();
        info.id = default.clone();
        Self::new(provider, default.clone(), [(default, value, info)])
    }

    /// Resolve the request's account, falling back to the default.
    pub fn resolve(&self, requested: Option<&AccountId>) -> Result<&Arc<T>, PaymentError> {
        let id = requested.unwrap_or(&self.default);
        self.accounts.get(id).ok_or_else(|| {
            PaymentError::validation(format!("unknown {} account `{id}`", self.provider))
        })
    }

    /// Default account id.
    pub fn default_id(&self) -> &AccountId {
        &self.default
    }

    /// Public account views, in configuration order.
    pub fn accounts(&self) -> &[PaymentAccount] {
        &self.public
    }

    /// Account ids.
    pub fn ids(&self) -> impl Iterator<Item = &AccountId> {
        self.accounts.keys()
    }

    /// Iterate over `(account id, credentials)` pairs.
    ///
    /// Adapters use this for the frontend payment catalog; the public
    /// [`Self::accounts`] view remains the only secret-free account projection.
    pub fn iter(&self) -> impl Iterator<Item = (&AccountId, &Arc<T>)> {
        self.accounts.iter()
    }

    /// Number of accounts.
    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    /// Whether no accounts are configured (never true after `new`).
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    /// Provider owning the set.
    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str) -> PaymentAccount {
        PaymentAccount {
            id: AccountId::from(id),
            provider: ProviderId::Stripe,
            live: false,
            label: None,
        }
    }

    #[test]
    fn resolves_default_and_explicit_accounts() {
        let set = AccountSet::new(
            ProviderId::Stripe,
            AccountId::from("a"),
            [
                (AccountId::from("a"), 1u32, info("a")),
                (AccountId::from("b"), 2u32, info("b")),
            ],
        )
        .unwrap();
        assert_eq!(**set.resolve(None).unwrap(), 1);
        assert_eq!(**set.resolve(Some(&AccountId::from("b"))).unwrap(), 2);
        assert!(set.resolve(Some(&AccountId::from("c"))).is_err());
    }

    #[test]
    fn rejects_missing_default_and_duplicates() {
        assert!(AccountSet::new(
            ProviderId::Stripe,
            AccountId::from("missing"),
            [(AccountId::from("a"), 1u32, info("a"))],
        )
        .is_err());
        assert!(AccountSet::new(
            ProviderId::Stripe,
            AccountId::from("a"),
            [
                (AccountId::from("a"), 1u32, info("a")),
                (AccountId::from("a"), 2u32, info("a")),
            ],
        )
        .is_err());
    }
}
