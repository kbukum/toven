//! Resolved release targets keyed per federation member and ecosystem.

use std::collections::BTreeMap;

use toven_model::{EcosystemId, MemberId};
use toven_ports::ReleaseAdapter;

/// Release adapters resolved per `(member, ecosystem)`.
///
/// Keying by member as well as ecosystem keeps each federation member's
/// publishability authoritative: two members exposing the same ecosystem (e.g.
/// `rust`) can disagree on `publish`, so a publishable member must never cause
/// a `publish = false` member's modules in that ecosystem to be released. The
/// single-repo case is one entry under the `None` member.
#[allow(clippy::redundant_pub_crate)]
pub(crate) type ReleaseTargets = BTreeMap<(Option<MemberId>, EcosystemId), Box<dyn ReleaseAdapter>>;
