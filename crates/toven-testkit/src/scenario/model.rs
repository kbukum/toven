use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;
use serde::de::{self, Visitor};

/// A declarative end-to-end scenario: one session of `toven` invocations run
/// inside a materialized fixture repo, with golden expectations per step.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Fixture repo id — a path under the shared `fixtures/repos/` tree.
    pub repo: String,
    /// Toolchain gates; empty (or omitted) means the scenario is planning-only
    /// and needs no real toolchain.
    #[serde(default)]
    pub requires: Vec<Requires>,
    /// Environment overrides the engine applies to every step, *on top of* its
    /// deterministic base (pinned clock, scoped cache, `LC_ALL`, `TERM`) — a
    /// scenario that overrides a base pin does so deliberately and visibly.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Optional scripted git history materialized before any step runs.
    #[serde(default)]
    pub git: Option<GitScript>,
    /// The ordered `toven` invocations. Ordering is first-class: it is how
    /// cold → warm caching, idempotency, and "affected since" are exercised.
    pub steps: Vec<Step>,
}

/// A toolchain a scenario requires; absent toolchains skip the scenario green.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Requires {
    /// The Rust toolchain (`cargo`).
    Cargo,
    /// The Go toolchain (`go`).
    Go,
}

impl Requires {
    /// The program probed on `PATH` to satisfy this gate.
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Go => "go",
        }
    }
}

/// Scripted git history applied to the materialized repo with pinned
/// identity and dates.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitScript {
    /// Commits applied in order after the initial import commit.
    #[serde(default)]
    pub commits: Vec<GitCommit>,
    /// Tags created at `HEAD` after all commits.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Branches created at `HEAD` after all commits.
    #[serde(default)]
    pub branches: Vec<String>,
}

/// One scripted commit: touch the listed repo-relative files, then commit.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitCommit {
    /// The commit message.
    pub msg: String,
    /// Repo-relative files created or appended to before committing.
    #[serde(default)]
    pub touch: Vec<String>,
}

/// One `toven` invocation inside the session.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Stable id; golden files are named `<id>.stdout` / `<id>.stderr`.
    pub id: String,
    /// Arguments passed to `toven` verbatim — the engine never rewrites them.
    pub argv: Vec<String>,
    /// Optional config-variant filename inside the repo (passed as `--config`);
    /// omitted means Toven's default `toven.toml` discovery.
    #[serde(default)]
    pub config: Option<String>,
    /// Expected exit code (default `0`).
    #[serde(default)]
    pub exit: i32,
    /// Golden expectation for stdout; omitted means the stream is not asserted.
    #[serde(default)]
    pub stdout: Option<StreamExpectation>,
    /// Golden expectation for stderr; omitted means the stream is not asserted.
    #[serde(default)]
    pub stderr: Option<StreamExpectation>,
    /// Declarative side-effect assertions checked after the invocation.
    #[serde(default)]
    pub effects: Vec<Effect>,
}

/// How one captured stream is compared against its golden file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamExpectation {
    /// The matcher tier for this stream.
    #[serde(rename = "match")]
    pub matcher: MatcherKind,
    /// Leading lines matched positionally — `line-set` only.
    #[serde(default)]
    pub frame_prefix: Option<usize>,
    /// Trailing lines matched positionally — `line-set` only.
    #[serde(default)]
    pub frame_suffix: Option<usize>,
}

/// The matcher tiers a scenario may pick per stream, strictest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum MatcherKind {
    /// Byte-for-byte equality.
    Exact,
    /// Byte equality after the Toven default normalizer scrubs volatile tokens.
    Normalized,
    /// Positional frame plus order-insensitive middle band (parallel output).
    LineSet,
    /// Every non-blank expected line present, in order (noisy toolchain output).
    Subset,
}

/// A declarative side-effect assertion checked after a step runs.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Effect {
    /// The scenario-scoped cache holds a matching number of entries.
    CacheEntries(Cmp),
    /// The repo-relative path exists (any kind — file, directory, symlink).
    FileExists(String),
    /// The repo-relative file's content matches a golden file in the scenario
    /// directory.
    FileMatches {
        /// Repo-relative file to check.
        path: String,
        /// Golden filename inside the scenario directory.
        golden: String,
    },
    /// The repo-relative path does not exist (any kind).
    PathAbsent(String),
    /// The git tag exists in the materialized repo.
    GitTagExists(String),
    /// The git tag does not exist in the materialized repo — the tag-absence
    /// mirror of [`GitTagExists`](Self::GitTagExists) that proves a rehearsal
    /// or rejected mutation created no release tag.
    GitTagAbsent(String),
}

/// A count comparison: `3`, `">0"`, `">=2"`, `"<5"`, or `"<=1"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cmp {
    op: CmpOp,
    value: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CmpOp {
    Eq,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Cmp {
    /// Whether `count` satisfies this comparison.
    #[must_use]
    pub const fn matches(&self, count: u64) -> bool {
        match self.op {
            CmpOp::Eq => count == self.value,
            CmpOp::Gt => count > self.value,
            CmpOp::Ge => count >= self.value,
            CmpOp::Lt => count < self.value,
            CmpOp::Le => count <= self.value,
        }
    }

    fn parse(text: &str) -> Result<Self, String> {
        // Two-character operators first so ">=" is not read as ">" + "=2".
        const OPERATORS: [(&str, CmpOp); 5] = [
            (">=", CmpOp::Ge),
            ("<=", CmpOp::Le),
            ("==", CmpOp::Eq),
            (">", CmpOp::Gt),
            ("<", CmpOp::Lt),
        ];
        let trimmed = text.trim();
        let (op, rest) = OPERATORS
            .iter()
            .find_map(|(prefix, op)| trimmed.strip_prefix(prefix).map(|rest| (*op, rest)))
            .unwrap_or((CmpOp::Eq, trimmed));
        let digits = rest.trim();
        // Digits only (no `+`/`_` forms `u64::from_str` would tolerate),
        // matching the published JSON schema exactly.
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!(
                "invalid count comparison '{text}' (expected N, >N, >=N, <N, or <=N)"
            ));
        }
        digits
            .parse::<u64>()
            .map(|value| Self { op, value })
            .map_err(|_| {
                format!("invalid count comparison '{text}' (expected N, >N, >=N, <N, or <=N)")
            })
    }
}

impl fmt::Display for Cmp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let op = match self.op {
            CmpOp::Eq => "",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
        };
        write!(f, "{op}{}", self.value)
    }
}

impl<'de> Deserialize<'de> for Cmp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct CmpVisitor;

        impl Visitor<'_> for CmpVisitor {
            type Value = Cmp;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a count comparison like 3, \">0\", or \">=2\"")
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Cmp, E> {
                Ok(Cmp {
                    op: CmpOp::Eq,
                    value,
                })
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Cmp, E> {
                u64::try_from(value)
                    .map(|value| Cmp {
                        op: CmpOp::Eq,
                        value,
                    })
                    .map_err(|_| E::custom("count comparison must be non-negative"))
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Cmp, E> {
                Cmp::parse(text).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(CmpVisitor)
    }
}
