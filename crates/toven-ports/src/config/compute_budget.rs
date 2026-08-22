//! Compute-budget policy — the engine-owned, config-selected CPU budget for
//! CPU-bound tool fan-out.

use std::fmt;
use std::num::NonZeroUsize;

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
/// share as an environment variable (never argv), for exactly the tasks whose
/// ecosystem registers a compute-budget env name (e.g. Go's `GOMAXPROCS`). A
/// task whose ecosystem registers none is left untouched and keeps its own
/// default parallelism — Cargo, for instance, exposes no such env knob, so it
/// opts out regardless of how many units share its wave.
///
/// Selected by config: `compute_budget = "auto"` (default), a positive integer
/// (`compute_budget = 8`), or `compute_budget = "inherit"` (`0` is the numeric
/// spelling of `inherit`). It is expressed once on `[toven]` and may be
/// overridden per `[ecosystems.<id>]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ComputeBudget {
    /// Size the total budget to the host's available CPUs and split it across
    /// the units running concurrently.
    #[default]
    Auto,
    /// A fixed total thread budget split across the concurrent units. The
    /// [`NonZeroUsize`] payload makes the documented `>= 1` invariant
    /// unrepresentable at zero — a zero input is [`Inherit`](Self::Inherit),
    /// never a `Fixed(0)` that would disagree with its serde round-trip. Build
    /// one from a raw count with [`ComputeBudget::fixed`].
    Fixed(NonZeroUsize),
    /// Inject nothing: every tool keeps its own default parallelism (today's
    /// behavior). The opt-out escape hatch.
    Inherit,
}

impl ComputeBudget {
    /// Build a fixed budget from a raw thread count, mapping `0` to the
    /// [`Inherit`](Self::Inherit) opt-out (zero is the numeric spelling of
    /// `"inherit"`, so a zero can never construct an invalid `Fixed(0)`).
    #[must_use]
    pub const fn fixed(threads: usize) -> Self {
        match NonZeroUsize::new(threads) {
            Some(threads) => Self::Fixed(threads),
            None => Self::Inherit,
        }
    }

    /// Whether this is the default (`Auto`), so it can be skipped on serialize.
    #[must_use]
    pub const fn is_default(&self) -> bool {
        matches!(self, Self::Auto)
    }

    /// Resolve this budget into a total thread count, or `None` when it opts
    /// out of injection entirely ([`Inherit`](Self::Inherit)).
    ///
    /// `host_cpus` supplies the host-sized total for [`Auto`](Self::Auto) (and
    /// any future host-sized mode); it is injected so this stays pure and
    /// deterministic in tests. The match is **exhaustive**: adding a sizing
    /// variant to this enum will not compile until its resolution is defined
    /// here, so a new mode can never fall through to a success-shaped default
    /// that silently gives it the wrong budget.
    #[must_use]
    pub fn total_threads(self, host_cpus: impl FnOnce() -> usize) -> Option<NonZeroUsize> {
        match self {
            Self::Inherit => None,
            Self::Fixed(threads) => Some(threads),
            // A host that cannot report its CPU count resolves to a single
            // thread rather than zero (which is not a valid budget).
            Self::Auto => Some(NonZeroUsize::new(host_cpus()).unwrap_or(NonZeroUsize::MIN)),
        }
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
            Self::Fixed(threads) => serializer.serialize_u64(threads.get() as u64),
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
        // Zero is the numeric spelling of the opt-out, matching `"inherit"`;
        // `ComputeBudget::fixed` folds it into `Inherit` for us.
        usize::try_from(value)
            .map(ComputeBudget::fixed)
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
    use std::num::NonZeroUsize;

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
        assert_eq!(parse("8"), ComputeBudget::fixed(8));
        assert_eq!(parse("1"), ComputeBudget::fixed(1));
    }

    #[test]
    fn zero_is_the_numeric_opt_out() {
        assert_eq!(parse("0"), ComputeBudget::Inherit);
        // The constructor folds zero into `Inherit` too, so a `Fixed(0)` that
        // would disagree with its serde round-trip is unrepresentable.
        assert_eq!(ComputeBudget::fixed(0), ComputeBudget::Inherit);
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
            ComputeBudget::fixed(6),
        ] {
            let holder = Holder { budget };
            let text = toml::to_string(&holder).expect("serializes");
            let back: Holder = toml::from_str(&text).expect("re-parses");
            assert_eq!(back.budget, budget, "round-trip via {text:?}");
        }
    }

    #[test]
    fn total_threads_resolves_each_variant() {
        // `Inherit` opts out; `Fixed` keeps its count; `Auto` takes the injected
        // host size. The host closure is injected so the result is deterministic.
        assert_eq!(ComputeBudget::Inherit.total_threads(|| 8), None);
        assert_eq!(
            ComputeBudget::fixed(5)
                .total_threads(|| 8)
                .map(NonZeroUsize::get),
            Some(5),
        );
        assert_eq!(
            ComputeBudget::Auto
                .total_threads(|| 8)
                .map(NonZeroUsize::get),
            Some(8),
        );
    }

    #[test]
    fn auto_falls_back_to_one_when_the_host_reports_zero_cpus() {
        // A zero host count is not a valid budget; `Auto` resolves to a single
        // thread rather than producing `NonZeroUsize::new(0) == None`.
        assert_eq!(
            ComputeBudget::Auto
                .total_threads(|| 0)
                .map(NonZeroUsize::get),
            Some(1),
        );
    }
}
