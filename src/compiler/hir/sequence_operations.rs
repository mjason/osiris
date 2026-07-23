#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CollectionOperation {
    Map,
    Mapcat,
    Mapcatv,
    Filter,
    Filterv,
}

/// Sequence helpers share one small lowering path.  They remain ordinary
/// runtime functions in the Python prelude; the enum only gives typed HIR a
/// stable contract for their callback and result shapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SequenceOperation {
    Cons,
    Concat,
    Count,
    EmptyQ,
    SeqQ,
    CollQ,
    SequentialQ,
    First,
    Rest,
    Next,
    Nth,
    Seq,
    Empty,
    Take,
    Drop,
    TakeWhile,
    DropWhile,
    Keep,
    KeepIndexed,
    Remove,
    Removev,
    Distinct,
    Dedupe,
    Partition,
    PartitionAll,
    PartitionBy,
    Interleave,
    Interpose,
    TakeLast,
    DropLast,
    MapIndexed,
    Iterate,
    Repeat,
    Repeatedly,
    Cycle,
    Sequence,
    Reductions,
    RunBang,
    Doall,
    Dorun,
    Some,
    Every,
    NotEvery,
    NotAny,
}

impl SequenceOperation {
    pub(super) fn from_source_name(name: &str) -> Option<Self> {
        Some(match name {
            "cons" => Self::Cons,
            "concat" => Self::Concat,
            "count" => Self::Count,
            "empty?" => Self::EmptyQ,
            "seq?" => Self::SeqQ,
            "coll?" => Self::CollQ,
            "sequential?" => Self::SequentialQ,
            "first" => Self::First,
            "rest" => Self::Rest,
            "next" => Self::Next,
            "nth" => Self::Nth,
            "seq" => Self::Seq,
            "empty" => Self::Empty,
            "take" => Self::Take,
            "drop" => Self::Drop,
            "take-while" => Self::TakeWhile,
            "drop-while" => Self::DropWhile,
            "keep" => Self::Keep,
            "keep-indexed" => Self::KeepIndexed,
            "remove" => Self::Remove,
            "removev" => Self::Removev,
            "distinct" => Self::Distinct,
            "dedupe" => Self::Dedupe,
            "partition" => Self::Partition,
            "partition-all" => Self::PartitionAll,
            "partition-by" => Self::PartitionBy,
            "interleave" => Self::Interleave,
            "interpose" => Self::Interpose,
            "take-last" => Self::TakeLast,
            "drop-last" => Self::DropLast,
            "map-indexed" => Self::MapIndexed,
            "iterate" => Self::Iterate,
            "repeat" => Self::Repeat,
            "repeatedly" => Self::Repeatedly,
            "cycle" => Self::Cycle,
            "sequence" => Self::Sequence,
            "reductions" => Self::Reductions,
            "run!" => Self::RunBang,
            "doall" => Self::Doall,
            "dorun" => Self::Dorun,
            "some" => Self::Some,
            "every?" => Self::Every,
            "not-every?" => Self::NotEvery,
            "not-any?" => Self::NotAny,
            _ => return None,
        })
    }

    pub(super) fn runtime_name(self) -> &'static str {
        match self {
            Self::Cons => "cons",
            Self::Concat => "concat",
            Self::Count => "count",
            Self::EmptyQ => "empty_q",
            Self::SeqQ => "seq_q",
            Self::CollQ => "coll_q",
            Self::SequentialQ => "sequential_q",
            Self::First => "first",
            Self::Rest => "rest",
            Self::Next => "next",
            Self::Nth => "nth",
            Self::Seq => "seq",
            Self::Empty => "empty",
            Self::Take => "take",
            Self::Drop => "drop",
            Self::TakeWhile => "take_while",
            Self::DropWhile => "drop_while",
            Self::Keep => "keep",
            Self::KeepIndexed => "keep_indexed",
            Self::Remove => "remove",
            Self::Removev => "removev",
            Self::Distinct => "distinct",
            Self::Dedupe => "dedupe",
            Self::Partition => "partition",
            Self::PartitionAll => "partition_all",
            Self::PartitionBy => "partition_by",
            Self::Interleave => "interleave",
            Self::Interpose => "interpose",
            Self::TakeLast => "take_last",
            Self::DropLast => "drop_last",
            Self::MapIndexed => "map_indexed",
            Self::Iterate => "iterate",
            Self::Repeat => "repeat",
            Self::Repeatedly => "repeatedly",
            Self::Cycle => "cycle",
            Self::Sequence => "sequence",
            Self::Reductions => "reductions",
            Self::RunBang => "run_bang",
            Self::Doall => "doall",
            Self::Dorun => "dorun",
            Self::Some => "some",
            Self::Every => "every_q",
            Self::NotEvery => "not_every_q",
            Self::NotAny => "not_any_q",
        }
    }

    pub(super) fn accepts_arity(self, arity: usize) -> bool {
        match self {
            Self::Concat => true,
            Self::Interleave => arity >= 2,
            Self::Partition => (2..=4).contains(&arity),
            Self::PartitionAll => (2..=3).contains(&arity),
            Self::Repeat | Self::Repeatedly | Self::DropLast | Self::Doall | Self::Dorun => {
                (1..=2).contains(&arity)
            }
            Self::Nth | Self::Reductions => (2..=3).contains(&arity),
            Self::Cons
            | Self::Take
            | Self::Drop
            | Self::TakeWhile
            | Self::DropWhile
            | Self::Keep
            | Self::KeepIndexed
            | Self::Remove
            | Self::Removev
            | Self::PartitionBy
            | Self::Interpose
            | Self::TakeLast
            | Self::MapIndexed
            | Self::Iterate
            | Self::RunBang
            | Self::Some
            | Self::Every
            | Self::NotEvery
            | Self::NotAny => arity == 2,
            Self::Count
            | Self::EmptyQ
            | Self::SeqQ
            | Self::CollQ
            | Self::SequentialQ
            | Self::First
            | Self::Rest
            | Self::Next
            | Self::Seq
            | Self::Empty
            | Self::Cycle
            | Self::Distinct
            | Self::Dedupe
            | Self::Sequence => arity == 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ControlIntrinsic {
    Truthy,
    Nil,
    Present,
    Nonempty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReducedOperation {
    Wrap,
    Predicate,
    Unwrap,
}

impl ReducedOperation {
    pub(super) fn runtime_name(self) -> &'static str {
        match self {
            Self::Wrap => "reduced",
            Self::Predicate => "reduced_p",
            Self::Unwrap => "unreduced",
        }
    }
}

impl ControlIntrinsic {
    pub(super) fn runtime_name(self) -> &'static str {
        match self {
            Self::Truthy => "truthy",
            Self::Nil => "is_nil",
            Self::Present => "present",
            Self::Nonempty => "nonempty",
        }
    }
}

impl CollectionOperation {
    pub(super) fn from_source_name(name: &str) -> Option<Self> {
        Some(match name {
            "map" => Self::Map,
            "mapcat" => Self::Mapcat,
            "mapcatv" => Self::Mapcatv,
            "filter" => Self::Filter,
            "filterv" => Self::Filterv,
            _ => return None,
        })
    }

    pub(super) fn runtime_name(self) -> &'static str {
        match self {
            Self::Map => "map",
            Self::Mapcat => "mapcat",
            Self::Mapcatv => "mapcatv",
            Self::Filter => "filter",
            Self::Filterv => "filterv",
        }
    }

    pub(super) fn result_is_vector(self) -> bool {
        matches!(self, Self::Mapcatv | Self::Filterv)
    }
}
