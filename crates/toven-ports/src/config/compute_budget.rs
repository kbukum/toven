//! Compute-budget policy — the engine-owned, config-selected CPU budget for
//! CPU-bound tool fan-out.

use std::fmt;

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

/// How much CPU parallelism the engine hands each spawned tool.
///
/// A per-module (`PerModule`) task fans out into one child process per module,
/// and the worker pool runs several at once; left unbounded, each child also
/// defaults its own internal parallelism to the full core count, so peak thread
/// pressure approaches cores². This policy caps that: the engine divides a total
/// budget across the units running concurrently and injects the per-process
/// share as an environment variable (never argv). A `Batchable` task is a single
/// self-balancing invocation, so the divisor is naturally one and it keeps the
/// whole budget.
///
/// Selected by config: `compute_budget = "auto"` (default), a positive integer
/// (`compute_budget = 8`), or `compute_budget = "inherit"`. It is expressed once
/// on `[toven]` and may be overridden per `[ecosystems.<id>]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ComputeBudget {
    /// Size the total budget to the host's available CPUs and split it across
    /// the units running concurrently.
    #[default]
    Auto,
    /// A fixed total thread budget (`>= 1`) split across the concurrent units.
    Fixed(usize),
    /// Inject nothing: every tool keeps its own default parallelism (today's
    /// behavior). The opt-out escape hatch.
    Inherit,
}

impl ComputeBudget {
    /// Whether this is the default (`Auto`), so it can be skipped on serialize.
    #[must_use]
    pub const fn is_default(&self) -> bool {
        matches!(self, Self::Auto)
    }
}

impl fmt::Display for ComputeBudget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("auto"),
            Self::Inherit => formatter.write_str("inherit"),
            Self::Fixed(threads) => write!(formatter, "{threads}"),
        }
    }
}

/// The word (`"auto"`) selecting the host-sized budget.
const AUTO: &str = "auto";
/// The word (`"inherit"`) selecting the no-injection opt-out.
const INHERIT: &str = "inherit";

impl Serialize for ComputeBudget {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str(AUTO),
            Self::Inherit => serializer.serialize_str(INHERIT),
            // A `usize` always fits `u64` on the platforms Toven targets.
            Self::Fixed(threads) => serializer.serialize_u64(*threads as u64),
        }
    }
}

impl<'de> Deserialize<'de> for ComputeBudget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(BudgetVisitor)
    }
}

/// Accepts either the reserved words or a non-negative integer thread count.
struct BudgetVisitor;

impl de::Visitor<'_> for BudgetVisitor {
    type Value = ComputeBudget;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str(r#""auto", "inherit", or a non-negative integer thread count (0 = inherit)"#)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<ComputeBudget, E> {
        match value {
            AUTO => Ok(ComputeBudget::Auto),
            INHERIT => Ok(ComputeBudget::Inherit),
            other => Err(de::Error::invalid_value(de::Unexpected::Str(other), &self)),
        }
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<ComputeBudget, E> {
        // Zero is the numeric spelling of the opt-out, matching `"inherit"`.
        if value == 0 {
            return Ok(ComputeBudget::Inherit);
        }
        usize::try_from(value)
            .map(ComputeBudget::Fixed)
            .map_err(|_| de::Error::invalid_value(de::Unexpected::Unsigned(value), &self))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<ComputeBudget, E> {
        u64::try_from(value)
            .map_err(|_| de::Error::invalid_value(de::Unexpected::Signed(value), &self))
            .and_then(|unsigned| self.visit_u64(unsigned))
    }
}

#[cfg(test)]
mod tests {
    use super::ComputeBudget;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Deserialize, Serialize)]
    struct Holder {
        budget: ComputeBudget,
    }

    fn parse(value: &str) -> ComputeBudget {
        toml::from_str::<Holder>(&format!("budget = {value}\n"))
            .expect("parses")
            .budget
    }

    #[test]
    fn default_is_auto() {
        assert_eq!(ComputeBudget::default(), ComputeBudget::Auto);
        assert!(ComputeBudget::Auto.is_default());
        assert!(!ComputeBudget::Inherit.is_default());
    }

    #[test]
    fn words_parse() {
        assert_eq!(parse("\"auto\""), ComputeBudget::Auto);
        assert_eq!(parse("\"inherit\""), ComputeBudget::Inherit);
    }

    #[test]
    fn positive_integer_is_a_fixed_budget() {
        assert_eq!(parse("8"), ComputeBudget::Fixed(8));
        assert_eq!(parse("1"), ComputeBudget::Fixed(1));
    }

    #[test]
    fn zero_is_the_numeric_opt_out() {
        assert_eq!(parse("0"), ComputeBudget::Inherit);
    }

    #[test]
    fn an_unknown_word_is_rejected() {
        let error =
            toml::from_str::<Holder>("budget = \"turbo\"\n").expect_err("unknown word rejected");
        assert!(error.to_string().contains("turbo"), "{error}");
    }

    #[test]
    fn round_trips_through_toml() {
        for budget in [
            ComputeBudget::Auto,
            ComputeBudget::Inherit,
            ComputeBudget::Fixed(6),
        ] {
            let holder = Holder { budget };
            let text = toml::to_string(&holder).expect("serializes");
            let back: Holder = toml::from_str(&text).expect("re-parses");
            assert_eq!(back.budget, budget, "round-trip via {text:?}");
        }
    }
}
