use serde::Serialize;

use super::{data::DataProperties, effects::EffectRow, temporal::TemporalSummary, type_repr::Type};

/// The summaries produced by calling a function. They are latent: merely
/// evaluating a function value does not produce them.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct CallSummaries {
    pub effects: EffectRow,
    pub temporal: TemporalSummary,
    pub data: DataProperties,
}

impl CallSummaries {
    #[must_use]
    pub const fn pure_scalar() -> Self {
        Self {
            effects: EffectRow::pure(),
            temporal: TemporalSummary::pointwise(),
            data: DataProperties::scalar(),
        }
    }

    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            effects: EffectRow::unknown(),
            temporal: TemporalSummary::unknown(),
            data: DataProperties::unknown(),
        }
    }

    #[must_use]
    pub fn join(&self, other: &Self) -> Self {
        Self {
            effects: self.effects.union(&other.effects),
            temporal: self.temporal.join(&other.temporal),
            data: self.data.join(&other.data),
        }
    }

    #[must_use]
    pub(in crate::types) fn is_within(&self, allowed: &Self) -> bool {
        self.effects.is_within(&allowed.effects)
            && self.temporal.is_within(&allowed.temporal)
            && self.data.is_within(&allowed.data)
    }
}

impl Default for CallSummaries {
    fn default() -> Self {
        Self::pure_scalar()
    }
}

/// A callable's type and the summaries produced when it is invoked.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct FunctionType {
    pub parameters: Vec<Type>,
    pub return_type: Box<Type>,
    pub summaries: CallSummaries,
}

impl FunctionType {
    #[must_use]
    pub fn new(parameters: Vec<Type>, return_type: Type) -> Self {
        Self {
            parameters,
            return_type: Box::new(return_type),
            summaries: CallSummaries::default(),
        }
    }

    #[must_use]
    pub fn with_summaries(mut self, summaries: CallSummaries) -> Self {
        self.summaries = summaries;
        self
    }
}
