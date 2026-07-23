use std::collections::BTreeSet;

use serde::Serialize;

/// A runtime effect produced when a function value is called.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    Io,
    Throw,
    Mutation,
    HiddenState,
    PythonDynamic,
    Custom(String),
}

/// A closed row enumerates every effect. An open row conservatively permits
/// effects which are not yet known (for example at a dynamic Python boundary).
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct EffectRow {
    pub effects: BTreeSet<Effect>,
    pub open: bool,
}

impl EffectRow {
    #[must_use]
    pub const fn pure() -> Self {
        Self {
            effects: BTreeSet::new(),
            open: false,
        }
    }

    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            effects: BTreeSet::new(),
            open: true,
        }
    }

    #[must_use]
    pub fn singleton(effect: Effect) -> Self {
        Self {
            effects: BTreeSet::from([effect]),
            open: false,
        }
    }

    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            effects: self.effects.union(&other.effects).cloned().collect(),
            open: self.open || other.open,
        }
    }

    /// Whether a function with this effect row may be used where `allowed` is
    /// expected. An open expected row accepts additional effects; an open
    /// actual row cannot satisfy a closed expectation.
    #[must_use]
    pub fn is_within(&self, allowed: &Self) -> bool {
        allowed.open || (!self.open && self.effects.is_subset(&allowed.effects))
    }
}
