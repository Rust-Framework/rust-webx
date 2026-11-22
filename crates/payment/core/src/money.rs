//! Currency and integer minor-unit amounts.
//!
//! Payment APIs disagree on representation (Stripe/Adyen/Square use integer
//! minor units, Alipay uses a decimal yuan string, PayPal uses a decimal string
//! plus a currency code). The shared model uses integer minor units only and
//! converts at the adapter boundary, so no amount ever passes through `f64`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// ISO 4217 currency, stored as three uppercase ASCII letters.
///
/// Any well-formed ISO code is accepted (the standard has ~180 entries); the
/// exponent table below decides how many decimals the minor unit has. Unknown
/// codes fall back to two decimals, matching the ISO default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Currency([u8; 3]);

impl Currency {
    /// Parse three ASCII letters (surrounding whitespace is trimmed).
    pub fn new(code: &str) -> Result<Self, MoneyError> {
        let code = code.trim();
        if code.len() != 3 || !code.is_ascii() {
            return Err(MoneyError::InvalidCurrency(code.to_string()));
        }
        let bytes = code.as_bytes();
        let mut out = [0u8; 3];
        for (index, byte) in bytes.iter().enumerate() {
            if !byte.is_ascii_alphabetic() {
                return Err(MoneyError::InvalidCurrency(code.to_string()));
            }
            out[index] = byte.to_ascii_uppercase();
        }
        Ok(Self(out))
    }

    /// The three-letter code.
    pub fn code(&self) -> &str {
        // SAFETY: the constructor/deserializer only admit ASCII letters.
        std::str::from_utf8(&self.0).unwrap_or("XXX")
    }

    /// Number of decimal digits in one major unit.
    ///
    /// `0` for JPY/KRW/VND/..., `3` for BHD/KWD/TND/..., `2` for everything
    /// else (including unknown codes).
    pub fn exponent(&self) -> u8 {
        match self.code() {
            "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF"
            | "UGX" | "UYI" | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
            "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
            "CLF" | "UYW" => 4,
            _ => 2,
        }
    }

    /// Whether one major unit is a whole number of minor units (`exponent == 0`).
    pub fn is_zero_decimal(&self) -> bool {
        self.exponent() == 0
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for Currency {
    type Err = MoneyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for Currency {
    type Error = MoneyError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<Currency> for String {
    fn from(value: Currency) -> Self {
        value.code().to_string()
    }
}

macro_rules! currency_constants {
    ($($name:ident => $code:literal),* $(,)?) => {
        impl Currency {
            $(
                #[doc = concat!("`", stringify!($code), "`")]
                #[allow(non_upper_case_globals)]
                pub const $name: Currency = Currency(*$code);
            )*
        }
    };
}

currency_constants! {
    CNY => b"CNY",
    USD => b"USD",
    EUR => b"EUR",
    GBP => b"GBP",
    HKD => b"HKD",
    SGD => b"SGD",
    JPY => b"JPY",
    KRW => b"KRW",
    TWD => b"TWD",
    AUD => b"AUD",
    CAD => b"CAD",
    CHF => b"CHF",
    MYR => b"MYR",
    THB => b"THB",
    VND => b"VND",
    PHP => b"PHP",
    IDR => b"IDR",
    INR => b"INR",
    AED => b"AED",
    SAR => b"SAR",
    KWD => b"KWD",
    BHD => b"BHD",
}

/// Money in integer minor units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Money {
    /// ISO 4217 currency.
    pub currency: Currency,
    /// Amount in the smallest currency unit (cent, fen, yen, fils, ...).
    pub minor: i64,
}

impl Money {
    /// Build an amount from minor units.
    pub fn new(currency: Currency, minor: i64) -> Self {
        Self { currency, minor }
    }

    /// Zero of `currency`.
    pub fn zero(currency: Currency) -> Self {
        Self::new(currency, 0)
    }

    /// CNY amount from fen.
    pub fn cny(fen: i64) -> Self {
        Self::new(Currency::CNY, fen)
    }

    /// USD amount from cents.
    pub fn usd(cents: i64) -> Self {
        Self::new(Currency::USD, cents)
    }

    /// Parse a decimal string such as `"99.00"` for `currency`.
    ///
    /// The number of fraction digits must not exceed the currency exponent;
    /// `"1.005"` in CNY is rejected instead of being silently rounded.
    pub fn from_major_str(currency: Currency, value: &str) -> Result<Self, MoneyError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(MoneyError::InvalidAmount(value.to_string()));
        }
        let (negative, digits) = match value.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, value),
        };
        let (major, fraction) = match digits.split_once('.') {
            Some((major, fraction)) => (major, Some(fraction)),
            None => (digits, None),
        };
        if major.is_empty() || !major.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(MoneyError::InvalidAmount(value.to_string()));
        }
        let exponent = currency.exponent() as usize;
        let fraction = fraction.unwrap_or("");
        if fraction.len() > exponent || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(MoneyError::ScaleExceeded {
                currency: currency.to_string(),
                exponent: currency.exponent(),
                value: value.to_string(),
            });
        }
        let major: i64 = major
            .parse()
            .map_err(|_| MoneyError::InvalidAmount(value.to_string()))?;
        let mut padded = fraction.to_string();
        while padded.len() < exponent {
            padded.push('0');
        }
        let fraction: i64 = if padded.is_empty() {
            0
        } else {
            padded
                .parse()
                .map_err(|_| MoneyError::InvalidAmount(value.to_string()))?
        };
        let scale = 10i64
            .checked_pow(exponent as u32)
            .ok_or_else(|| MoneyError::InvalidAmount(value.to_string()))?;
        let minor = major
            .checked_mul(scale)
            .and_then(|value| value.checked_add(fraction))
            .ok_or(MoneyError::Overflow)?;
        Ok(Self::new(currency, if negative { -minor } else { minor }))
    }

    /// Render the amount as a decimal string with exactly `exponent` decimals
    /// (`"99.00"` for CNY, `"1000"` for JPY).
    pub fn to_major_string(&self) -> String {
        let exponent = self.currency.exponent() as u32;
        let scale = 10i64.pow(exponent);
        let negative = self.minor < 0;
        let absolute = self.minor.unsigned_abs();
        let major = absolute / scale as u64;
        let fraction = absolute % scale as u64;
        let mut out = String::new();
        if negative {
            out.push('-');
        }
        out.push_str(&major.to_string());
        if exponent > 0 {
            out.push('.');
            let digits = format!("{:0width$}", fraction, width = exponent as usize);
            out.push_str(&digits);
        }
        out
    }

    /// Checked addition; different currencies are an error.
    pub fn checked_add(self, other: Self) -> Result<Self, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch {
                left: self.currency,
                right: other.currency,
            });
        }
        self.minor
            .checked_add(other.minor)
            .map(|minor| Self::new(self.currency, minor))
            .ok_or(MoneyError::Overflow)
    }

    /// Checked subtraction; different currencies are an error.
    pub fn checked_sub(self, other: Self) -> Result<Self, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch {
                left: self.currency,
                right: other.currency,
            });
        }
        self.minor
            .checked_sub(other.minor)
            .map(|minor| Self::new(self.currency, minor))
            .ok_or(MoneyError::Overflow)
    }

    /// Whether the amount is zero.
    pub fn is_zero(&self) -> bool {
        self.minor == 0
    }

    /// Whether the amount is strictly positive.
    pub fn is_positive(&self) -> bool {
        self.minor > 0
    }

    /// Same currency check.
    pub fn same_currency(&self, other: &Self) -> bool {
        self.currency == other.currency
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.to_major_string(), self.currency)
    }
}

/// Money parsing failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MoneyError {
    /// Not three ASCII letters.
    #[error("invalid ISO 4217 currency: {0}")]
    InvalidCurrency(String),
    /// Not a decimal number.
    #[error("invalid amount: {0}")]
    InvalidAmount(String),
    /// More fraction digits than the currency exponent allows.
    #[error("amount {value} has more than {exponent} decimal places for {currency}")]
    ScaleExceeded {
        /// Currency code.
        currency: String,
        /// Allowed fraction digits.
        exponent: u8,
        /// Rejected input.
        value: String,
    },
    /// `i64` overflow while scaling.
    #[error("amount overflow")]
    Overflow,
    /// Arithmetic across two currencies.
    #[error("currency mismatch: {left} vs {right}")]
    CurrencyMismatch {
        /// Left currency.
        left: Currency,
        /// Right currency.
        right: Currency,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_renders_two_decimal_currencies() {
        let money = Money::from_major_str(Currency::CNY, "99.5").unwrap();
        assert_eq!(money.minor, 9950);
        assert_eq!(money.to_major_string(), "99.50");

        let money = Money::from_major_str(Currency::USD, "0.01").unwrap();
        assert_eq!(money.minor, 1);
        assert_eq!(money.to_major_string(), "0.01");
    }

    #[test]
    fn zero_decimal_currency_has_no_fraction() {
        let money = Money::from_major_str(Currency::JPY, "1000").unwrap();
        assert_eq!(money.minor, 1000);
        assert_eq!(money.to_major_string(), "1000");
        assert!(Money::from_major_str(Currency::JPY, "10.5").is_err());
    }

    #[test]
    fn three_decimal_currency_keeps_scale() {
        let money = Money::from_major_str(Currency::KWD, "1.234").unwrap();
        assert_eq!(money.minor, 1234);
        assert_eq!(money.to_major_string(), "1.234");
    }

    #[test]
    fn rejects_scale_that_would_round() {
        let error = Money::from_major_str(Currency::CNY, "1.005").unwrap_err();
        assert!(matches!(error, MoneyError::ScaleExceeded { .. }));
        assert!(Money::from_major_str(Currency::CNY, "1.2.3").is_err());
        assert!(Money::from_major_str(Currency::CNY, "").is_err());
        assert!(Money::from_major_str(Currency::CNY, "1e3").is_err());
    }

    #[test]
    fn negative_amounts_roundtrip() {
        let money = Money::from_major_str(Currency::USD, "-12.34").unwrap();
        assert_eq!(money.minor, -1234);
        assert_eq!(money.to_major_string(), "-12.34");
    }

    #[test]
    fn currency_mismatch_is_an_error() {
        let cny = Money::cny(100);
        let usd = Money::usd(100);
        assert!(cny.checked_add(usd).is_err());
        assert!(!cny.same_currency(&usd));
    }

    #[test]
    fn currency_serde_roundtrip() {
        let text = serde_json::to_string(&Currency::CNY).unwrap();
        assert_eq!(text, "\"CNY\"");
        let currency: Currency = serde_json::from_str("\"cny\"").unwrap();
        assert_eq!(currency, Currency::CNY);
    }
}
