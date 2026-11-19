//! Cost calculation for usage events.
//!
//! Prices are integer micro-USD per **million** tokens, so a $0.15/M prompt
//! price is `150_000`. Cached prompt tokens and reasoning tokens are priced at
//! their own rates and are excluded from the base rate to avoid double billing.

use crate::usage::Usage;
use std::collections::HashMap;

/// Per-model price table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModelPrice {
    /// Uncached prompt tokens, micro-USD per million.
    pub prompt_micros_per_million: u64,
    /// Completion tokens that are not reasoning, micro-USD per million.
    pub completion_micros_per_million: u64,
    /// Cached prompt tokens, micro-USD per million.
    pub cached_micros_per_million: u64,
    /// Reasoning tokens, micro-USD per million.
    pub reasoning_micros_per_million: u64,
}

impl ModelPrice {
    /// All-zero price (free).
    pub const ZERO: Self = Self {
        prompt_micros_per_million: 0,
        completion_micros_per_million: 0,
        cached_micros_per_million: 0,
        reasoning_micros_per_million: 0,
    };

    /// Build a price.
    pub const fn new(
        prompt_micros_per_million: u64,
        completion_micros_per_million: u64,
        cached_micros_per_million: u64,
        reasoning_micros_per_million: u64,
    ) -> Self {
        Self {
            prompt_micros_per_million,
            completion_micros_per_million,
            cached_micros_per_million,
            reasoning_micros_per_million,
        }
    }

    /// Cost for one usage record, rounded up to the next micro-USD.
    pub fn cost_micros(&self, usage: &Usage) -> u64 {
        let uncached_prompt = usage.prompt_tokens.saturating_sub(usage.cached_tokens);
        let non_reasoning_completion = usage
            .completion_tokens
            .saturating_sub(usage.reasoning_tokens);
        let parts = [
            (uncached_prompt, self.prompt_micros_per_million),
            (non_reasoning_completion, self.completion_micros_per_million),
            (usage.cached_tokens, self.cached_micros_per_million),
            (usage.reasoning_tokens, self.reasoning_micros_per_million),
        ];
        parts.iter().fold(0u64, |total, (tokens, rate)| {
            total.saturating_add(tokens.saturating_mul(*rate).div_ceil(1_000_000))
        })
    }
}

/// Cost lookup used by the executor.
pub trait ICostCalculator: Send + Sync {
    /// Cost of `usage` for the public model alias, in micro-USD.
    fn cost_micros(&self, model: &str, usage: &Usage) -> u64;
}

/// Zero-cost calculator (default when pricing is not configured).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoCost;

impl ICostCalculator for NoCost {
    fn cost_micros(&self, _model: &str, _usage: &Usage) -> u64 {
        0
    }
}

/// Static per-model price table with an optional default.
#[derive(Debug, Clone, Default)]
pub struct StaticCostTable {
    prices: HashMap<String, ModelPrice>,
    default: ModelPrice,
}

impl StaticCostTable {
    /// Empty table (unknown models are free).
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a model price.
    pub fn with_price(mut self, model: impl Into<String>, price: ModelPrice) -> Self {
        self.prices.insert(model.into(), price);
        self
    }

    /// Price used for models not in the table.
    pub fn with_default_price(mut self, price: ModelPrice) -> Self {
        self.default = price;
        self
    }

    /// Number of explicit model prices.
    pub fn len(&self) -> usize {
        self.prices.len()
    }

    /// Whether the table has no explicit model prices.
    pub fn is_empty(&self) -> bool {
        self.prices.is_empty()
    }
}

impl ICostCalculator for StaticCostTable {
    fn cost_micros(&self, model: &str, usage: &Usage) -> u64 {
        self.prices
            .get(model)
            .unwrap_or(&self.default)
            .cost_micros(usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage() -> Usage {
        Usage {
            prompt_tokens: 1_000_000,
            completion_tokens: 500_000,
            cached_tokens: 200_000,
            reasoning_tokens: 100_000,
        }
    }

    #[test]
    fn prices_uncached_and_special_buckets_separately() {
        let price = ModelPrice::new(100_000, 400_000, 25_000, 600_000);
        // 800k * 0.1 + 400k * 0.4 + 200k * 0.025 + 100k * 0.6 = 80k + 160k + 5k + 60k
        assert_eq!(price.cost_micros(&usage()), 305_000);
    }

    #[test]
    fn table_uses_default_for_unknown_models() {
        let table = StaticCostTable::new()
            .with_price("fast", ModelPrice::new(1_000_000, 0, 0, 0))
            .with_default_price(ModelPrice::new(2_000_000, 0, 0, 0));
        let usage = Usage {
            prompt_tokens: 1,
            ..Usage::ZERO
        };
        assert_eq!(table.cost_micros("fast", &usage), 1);
        assert_eq!(table.cost_micros("other", &usage), 2);
        assert_eq!(NoCost.cost_micros("fast", &usage), 0);
    }

    #[test]
    fn zero_usage_is_free() {
        assert_eq!(ModelPrice::new(1, 1, 1, 1).cost_micros(&Usage::ZERO), 0);
    }
}
