use serde::Serialize;

/// Statically known alignment of data values.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Alignment {
    Positional,
    Labelled,
    AsOf,
    Unknown,
}

/// Conservative data-shape facts. `None` means that the fact is unknown, not
/// false. Domain extensions can later replace this with transfer expressions
/// while preserving the callable type layout.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct DataProperties {
    pub schema: Option<String>,
    pub axes: Option<Vec<String>>,
    pub alignment: Alignment,
    /// Keys which are statically known to be in ascending lexicographic order.
    pub ordered_by: Option<Vec<String>>,
    /// Keys which are statically known to identify rows uniquely.
    pub unique_by: Option<Vec<String>>,
    pub preserves_length: Option<bool>,
    pub materializes: Option<bool>,
    pub reshapes: Option<bool>,
    pub nulls_possible: Option<bool>,
    pub nan_possible: Option<bool>,
    pub nonfinite_possible: Option<bool>,
    pub nonfinite_policy: Option<String>,
}

impl DataProperties {
    #[must_use]
    pub const fn scalar() -> Self {
        Self {
            schema: None,
            axes: None,
            alignment: Alignment::Positional,
            ordered_by: None,
            unique_by: None,
            preserves_length: Some(true),
            materializes: Some(false),
            reshapes: Some(false),
            nulls_possible: Some(false),
            nan_possible: Some(false),
            nonfinite_possible: Some(false),
            nonfinite_policy: None,
        }
    }

    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            schema: None,
            axes: None,
            alignment: Alignment::Unknown,
            ordered_by: None,
            unique_by: None,
            preserves_length: None,
            materializes: None,
            reshapes: None,
            nulls_possible: None,
            nan_possible: None,
            nonfinite_possible: None,
            nonfinite_policy: None,
        }
    }

    #[must_use]
    pub fn join(&self, other: &Self) -> Self {
        Self {
            schema: equal_fact(&self.schema, &other.schema).flatten(),
            axes: equal_fact(&self.axes, &other.axes).flatten(),
            alignment: if self.alignment == other.alignment {
                self.alignment.clone()
            } else {
                Alignment::Unknown
            },
            ordered_by: equal_fact(&self.ordered_by, &other.ordered_by).flatten(),
            unique_by: equal_fact(&self.unique_by, &other.unique_by).flatten(),
            preserves_length: equal_fact(&self.preserves_length, &other.preserves_length).flatten(),
            materializes: equal_fact(&self.materializes, &other.materializes).flatten(),
            reshapes: equal_fact(&self.reshapes, &other.reshapes).flatten(),
            nulls_possible: equal_fact(&self.nulls_possible, &other.nulls_possible).flatten(),
            nan_possible: equal_fact(&self.nan_possible, &other.nan_possible).flatten(),
            nonfinite_possible: equal_fact(&self.nonfinite_possible, &other.nonfinite_possible)
                .flatten(),
            nonfinite_policy: equal_fact(&self.nonfinite_policy, &other.nonfinite_policy).flatten(),
        }
    }

    #[must_use]
    pub(super) fn is_within(&self, allowed: &Self) -> bool {
        fact_is_within(&self.schema, &allowed.schema)
            && fact_is_within(&self.axes, &allowed.axes)
            && (allowed.alignment == Alignment::Unknown || self.alignment == allowed.alignment)
            && fact_is_within(&self.ordered_by, &allowed.ordered_by)
            && fact_is_within(&self.unique_by, &allowed.unique_by)
            && fact_is_within(&self.preserves_length, &allowed.preserves_length)
            && fact_is_within(&self.materializes, &allowed.materializes)
            && fact_is_within(&self.reshapes, &allowed.reshapes)
            && fact_is_within(&self.nulls_possible, &allowed.nulls_possible)
            && fact_is_within(&self.nan_possible, &allowed.nan_possible)
            && fact_is_within(&self.nonfinite_possible, &allowed.nonfinite_possible)
            && fact_is_within(&self.nonfinite_policy, &allowed.nonfinite_policy)
    }
}

impl Default for DataProperties {
    fn default() -> Self {
        Self::scalar()
    }
}

fn equal_fact<T: Clone + PartialEq>(left: &T, right: &T) -> Option<T> {
    (left == right).then(|| left.clone())
}

fn fact_is_within<T: PartialEq>(actual: &Option<T>, allowed: &Option<T>) -> bool {
    allowed
        .as_ref()
        .is_none_or(|allowed| actual.as_ref() == Some(allowed))
}
