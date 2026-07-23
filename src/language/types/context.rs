use super::*;

/// Stateful local unification context. Failed unifications are transactional:
/// they do not leave partially bound variables behind.
#[derive(Clone, Debug, Default)]
pub struct TypeContext {
    next_variable: u32,
    substitutions: BTreeMap<TypeVarId, Type>,
}

impl TypeContext {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_variable: 0,
            substitutions: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn fresh_var(&mut self) -> Type {
        let variable = TypeVarId(self.next_variable);
        self.next_variable = self
            .next_variable
            .checked_add(1)
            .expect("too many type variables");
        Type::TypeVar(variable)
    }

    #[must_use]
    pub fn substitution(&self, variable: TypeVarId) -> Option<Type> {
        self.substitutions.get(&variable).map(|ty| self.resolve(ty))
    }

    /// Recursively applies all known substitutions to a type.
    #[must_use]
    pub fn resolve(&self, ty: &Type) -> Type {
        match ty {
            Type::TypeVar(variable) => self
                .substitutions
                .get(variable)
                .map_or_else(|| ty.clone(), |bound| self.resolve(bound)),
            Type::Option(inner) => Type::option(self.resolve(inner)),
            Type::Union(members) => Type::union(members.iter().map(|member| self.resolve(member))),
            Type::Tuple(members) => {
                Type::Tuple(members.iter().map(|member| self.resolve(member)).collect())
            }
            Type::List(item) => Type::List(Box::new(self.resolve(item))),
            Type::Vector(item) => Type::Vector(Box::new(self.resolve(item))),
            Type::Map(key, value) => {
                Type::Map(Box::new(self.resolve(key)), Box::new(self.resolve(value)))
            }
            Type::Set(item) => Type::Set(Box::new(self.resolve(item))),
            Type::Fn(function) => Type::Fn(FunctionType {
                parameters: function
                    .parameters
                    .iter()
                    .map(|parameter| self.resolve(parameter))
                    .collect(),
                return_type: Box::new(self.resolve(&function.return_type)),
                summaries: function.summaries.clone(),
            }),
            Type::Nominal { binding, args } => Type::Nominal {
                binding: binding.clone(),
                args: args.iter().map(|argument| self.resolve(argument)).collect(),
            },
            _ => ty.clone(),
        }
    }

    /// Unifies two inference types and returns their resolved common type.
    pub fn unify(&mut self, left: &Type, right: &Type) -> Result<Type, TypeError> {
        let checkpoint = self.substitutions.clone();
        match self.unify_inner(left, right) {
            Ok(ty) => Ok(self.resolve(&ty)),
            Err(error) => {
                self.substitutions = checkpoint;
                Err(error)
            }
        }
    }

    fn unify_inner(&mut self, left: &Type, right: &Type) -> Result<Type, TypeError> {
        let left = self.resolve(left);
        let right = self.resolve(right);
        if left == right {
            return Ok(left);
        }

        match (&left, &right) {
            (Type::Error, _) | (_, Type::Error) => Ok(Type::Error),
            (Type::TypeVar(variable), _) => self.bind(*variable, &right),
            (_, Type::TypeVar(variable)) => self.bind(*variable, &left),
            (Type::Any, _) | (_, Type::Any) => Err(TypeError::new(
                TypeErrorKind::AnyRequiresExplicitCast,
                left,
                right,
            )),
            (Type::Unknown, _) | (_, Type::Unknown) => Ok(Type::Unknown),
            (Type::Never, _) => Ok(right),
            (_, Type::Never) => Ok(left),
            (Type::Int, Type::Float) | (Type::Float, Type::Int) => Ok(Type::Float),
            (Type::Option(left), Type::Option(right)) => {
                Ok(Type::option(self.unify_inner(left, right)?))
            }
            (Type::Tuple(left), Type::Tuple(right)) => {
                Ok(Type::Tuple(self.unify_sequences(left, right)?))
            }
            (Type::List(left), Type::List(right)) => {
                Ok(Type::List(Box::new(self.unify_inner(left, right)?)))
            }
            (Type::Vector(left), Type::Vector(right)) => {
                Ok(Type::Vector(Box::new(self.unify_inner(left, right)?)))
            }
            (Type::Map(left_key, left_value), Type::Map(right_key, right_value)) => Ok(Type::Map(
                Box::new(self.unify_inner(left_key, right_key)?),
                Box::new(self.unify_inner(left_value, right_value)?),
            )),
            (Type::Set(left), Type::Set(right)) => {
                Ok(Type::Set(Box::new(self.unify_inner(left, right)?)))
            }
            (Type::Union(left), Type::Union(right)) => self.unify_unions(left, right),
            (Type::Fn(left), Type::Fn(right)) => self.unify_functions(left, right),
            (
                Type::Nominal {
                    binding: left_binding,
                    args: left_args,
                },
                Type::Nominal {
                    binding: right_binding,
                    args: right_args,
                },
            ) if left_binding == right_binding => Ok(Type::Nominal {
                binding: left_binding.clone(),
                args: self.unify_sequences(left_args, right_args)?,
            }),
            _ => Err(TypeError::mismatch(left, right)),
        }
    }

    fn bind(&mut self, variable: TypeVarId, ty: &Type) -> Result<Type, TypeError> {
        let ty = self.resolve(ty);
        if ty == Type::TypeVar(variable) {
            return Ok(ty);
        }
        if self.occurs(variable, &ty) {
            return Err(TypeError {
                kind: TypeErrorKind::OccursCheck { variable },
                expected: Some(Box::new(Type::TypeVar(variable))),
                found: Some(Box::new(ty)),
            });
        }
        self.substitutions.insert(variable, ty.clone());
        Ok(ty)
    }

    fn occurs(&self, variable: TypeVarId, ty: &Type) -> bool {
        match self.resolve(ty) {
            Type::TypeVar(candidate) => variable == candidate,
            Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
                self.occurs(variable, &inner)
            }
            Type::Union(members) | Type::Tuple(members) => {
                members.iter().any(|member| self.occurs(variable, member))
            }
            Type::Map(key, value) => self.occurs(variable, &key) || self.occurs(variable, &value),
            Type::Fn(function) => {
                function
                    .parameters
                    .iter()
                    .any(|parameter| self.occurs(variable, parameter))
                    || self.occurs(variable, &function.return_type)
            }
            Type::Nominal { args, .. } => {
                args.iter().any(|argument| self.occurs(variable, argument))
            }
            _ => false,
        }
    }

    fn unify_sequences(&mut self, left: &[Type], right: &[Type]) -> Result<Vec<Type>, TypeError> {
        if left.len() != right.len() {
            return Err(TypeError {
                kind: TypeErrorKind::ArityMismatch {
                    expected: left.len(),
                    found: right.len(),
                },
                expected: Some(Box::new(Type::Tuple(left.to_vec()))),
                found: Some(Box::new(Type::Tuple(right.to_vec()))),
            });
        }
        left.iter()
            .zip(right)
            .map(|(left, right)| self.unify_inner(left, right))
            .collect()
    }

    fn unify_unions(&mut self, left: &[Type], right: &[Type]) -> Result<Type, TypeError> {
        if left.len() != right.len() {
            return Err(TypeError::mismatch(
                Type::Union(left.to_vec()),
                Type::Union(right.to_vec()),
            ));
        }

        let checkpoint = self.substitutions.clone();
        let mut matched = vec![false; right.len()];
        if self.match_union_members(left, right, 0, &mut matched) {
            Ok(Type::union(left.iter().map(|member| self.resolve(member))))
        } else {
            self.substitutions = checkpoint;
            Err(TypeError::mismatch(
                Type::Union(left.to_vec()),
                Type::Union(right.to_vec()),
            ))
        }
    }

    fn match_union_members(
        &mut self,
        left: &[Type],
        right: &[Type],
        index: usize,
        matched: &mut [bool],
    ) -> bool {
        if index == left.len() {
            return true;
        }

        for candidate in 0..right.len() {
            if matched[candidate] {
                continue;
            }
            let checkpoint = self.substitutions.clone();
            if self.unify_inner(&left[index], &right[candidate]).is_ok() {
                matched[candidate] = true;
                if self.match_union_members(left, right, index + 1, matched) {
                    return true;
                }
                matched[candidate] = false;
            }
            self.substitutions = checkpoint;
        }
        false
    }

    fn unify_functions(
        &mut self,
        left: &FunctionType,
        right: &FunctionType,
    ) -> Result<Type, TypeError> {
        if left.summaries != right.summaries {
            return Err(TypeError {
                kind: TypeErrorKind::SummaryMismatch,
                expected: Some(Box::new(Type::Fn(left.clone()))),
                found: Some(Box::new(Type::Fn(right.clone()))),
            });
        }
        let parameters = self.unify_sequences(&left.parameters, &right.parameters)?;
        let return_type = self.unify_inner(&left.return_type, &right.return_type)?;
        Ok(Type::Fn(
            FunctionType::new(parameters, return_type).with_summaries(left.summaries.clone()),
        ))
    }

    /// Directional compatibility (`source` can flow into `target`). This is
    /// intentionally stricter than Python at an `Any -> T` boundary.
    #[must_use]
    pub fn is_assignable(&self, source: &Type, target: &Type) -> bool {
        self.check_assignable(source, target).is_ok()
    }

    /// Structured counterpart to [`Self::is_assignable`].
    pub fn check_assignable(&self, source: &Type, target: &Type) -> Result<(), TypeError> {
        let source = self.resolve(source);
        let target = self.resolve(target);
        if is_assignable_resolved(&source, &target) {
            Ok(())
        } else {
            let kind = if source == Type::Any && target != Type::Any {
                TypeErrorKind::AnyRequiresExplicitCast
            } else if matches!((&source, &target), (Type::Fn(_), Type::Fn(_))) {
                TypeErrorKind::SummaryOrFunctionMismatch
            } else {
                TypeErrorKind::Mismatch
            };
            Err(TypeError::new(kind, target, source))
        }
    }

    /// Least conservative type containing both inputs, used for branches and
    /// collection literals. Unlike unification this never binds variables.
    #[must_use]
    pub fn join(&self, left: &Type, right: &Type) -> Type {
        join_resolved(&self.resolve(left), &self.resolve(right))
    }

    #[must_use]
    pub fn join_all<'a>(&self, types: impl IntoIterator<Item = &'a Type>) -> Type {
        types
            .into_iter()
            .fold(Type::Never, |joined, ty| self.join(&joined, ty))
    }
}

fn is_assignable_resolved(source: &Type, target: &Type) -> bool {
    if source == target || source == &Type::Error || target == &Type::Error {
        return true;
    }
    if source == &Type::Never || target == &Type::Any {
        return true;
    }
    if source == &Type::Any {
        return false;
    }
    if source == &Type::Unknown || target == &Type::Unknown {
        return source == target;
    }
    if source == &Type::Int && target == &Type::Float {
        return true;
    }

    match (source, target) {
        (Type::None, Type::Option(_)) => true,
        (Type::Option(source), Type::Option(target)) => is_assignable_resolved(source, target),
        (source, Type::Option(target)) => {
            source != &Type::None && is_assignable_resolved(source, target)
        }
        (Type::Option(source), Type::Union(targets)) => {
            is_assignable_resolved(source, &Type::Union(targets.clone()))
                && targets.iter().any(|target| target == &Type::None)
        }
        (Type::Union(sources), target) => sources
            .iter()
            .all(|source| is_assignable_resolved(source, target)),
        (source, Type::Union(targets)) => targets
            .iter()
            .any(|target| is_assignable_resolved(source, target)),
        (Type::Tuple(sources), Type::Tuple(targets)) => {
            sources.len() == targets.len()
                && sources
                    .iter()
                    .zip(targets)
                    .all(|(source, target)| is_assignable_resolved(source, target))
        }
        (Type::List(source), Type::List(target))
        | (Type::Vector(source), Type::Vector(target))
        | (Type::Set(source), Type::Set(target)) => source == target,
        (Type::Map(source_key, source_value), Type::Map(target_key, target_value)) => {
            source_key == target_key && source_value == target_value
        }
        (Type::Fn(source), Type::Fn(target)) => function_is_assignable(source, target),
        (
            Type::Nominal {
                binding: source_binding,
                args: source_args,
            },
            Type::Nominal {
                binding: target_binding,
                args: target_args,
            },
        ) => source_binding == target_binding && source_args == target_args,
        (Type::TypeVar(source), Type::TypeVar(target)) => source == target,
        _ => false,
    }
}

fn function_is_assignable(source: &FunctionType, target: &FunctionType) -> bool {
    source.parameters.len() == target.parameters.len()
        && source
            .parameters
            .iter()
            .zip(&target.parameters)
            .all(|(source, target)| is_assignable_resolved(target, source))
        && is_assignable_resolved(&source.return_type, &target.return_type)
        && source.summaries.is_within(&target.summaries)
}

fn join_resolved(left: &Type, right: &Type) -> Type {
    if left == right {
        return left.clone();
    }
    match (left, right) {
        (Type::Error, _) | (_, Type::Error) => Type::Error,
        (Type::Unknown, _) | (_, Type::Unknown) => Type::Unknown,
        (Type::Any, _) | (_, Type::Any) => Type::Any,
        (Type::Never, other) | (other, Type::Never) => other.clone(),
        (Type::Int, Type::Float) | (Type::Float, Type::Int) => Type::Float,
        (Type::Option(left), Type::Option(right)) => Type::option(join_resolved(left, right)),
        (Type::List(left), Type::List(right)) => Type::List(Box::new(join_resolved(left, right))),
        (Type::Vector(left), Type::Vector(right)) => {
            Type::Vector(Box::new(join_resolved(left, right)))
        }
        (Type::Set(left), Type::Set(right)) => Type::Set(Box::new(join_resolved(left, right))),
        (Type::Map(left_key, left_value), Type::Map(right_key, right_value)) => Type::Map(
            Box::new(join_resolved(left_key, right_key)),
            Box::new(join_resolved(left_value, right_value)),
        ),
        (Type::Tuple(left), Type::Tuple(right)) if left.len() == right.len() => Type::Tuple(
            left.iter()
                .zip(right)
                .map(|(left, right)| join_resolved(left, right))
                .collect(),
        ),
        (Type::Fn(left), Type::Fn(right)) if left.parameters == right.parameters => Type::Fn(
            FunctionType::new(
                left.parameters.clone(),
                join_resolved(&left.return_type, &right.return_type),
            )
            .with_summaries(left.summaries.join(&right.summaries)),
        ),
        _ if is_assignable_resolved(left, right) => right.clone(),
        _ if is_assignable_resolved(right, left) => left.clone(),
        _ => Type::union([left.clone(), right.clone()]),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeError {
    pub kind: TypeErrorKind,
    pub expected: Option<Box<Type>>,
    pub found: Option<Box<Type>>,
}

impl TypeError {
    #[must_use]
    pub fn new(kind: TypeErrorKind, expected: Type, found: Type) -> Self {
        Self {
            kind,
            expected: Some(Box::new(expected)),
            found: Some(Box::new(found)),
        }
    }

    #[must_use]
    pub fn mismatch(expected: Type, found: Type) -> Self {
        Self::new(TypeErrorKind::Mismatch, expected, found)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeErrorKind {
    Mismatch,
    ArityMismatch { expected: usize, found: usize },
    OccursCheck { variable: TypeVarId },
    AnyRequiresExplicitCast,
    SummaryMismatch,
    SummaryOrFunctionMismatch,
}

impl fmt::Display for TypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.kind, &self.expected, &self.found) {
            (TypeErrorKind::OccursCheck { variable }, _, Some(found)) => {
                write!(
                    formatter,
                    "type variable ?{} occurs in `{found}`",
                    variable.0
                )
            }
            (TypeErrorKind::ArityMismatch { expected, found }, _, _) => {
                write!(
                    formatter,
                    "expected {expected} type arguments, found {found}"
                )
            }
            (TypeErrorKind::AnyRequiresExplicitCast, Some(expected), Some(found)) => write!(
                formatter,
                "cannot unify `{expected}` with `{found}` across an Any boundary without an explicit cast"
            ),
            (TypeErrorKind::SummaryMismatch, _, _) => {
                formatter.write_str("function latent summaries do not match")
            }
            (TypeErrorKind::SummaryOrFunctionMismatch, _, _) => {
                formatter.write_str("function signature or latent summaries are not assignable")
            }
            (_, Some(expected), Some(found)) => {
                write!(formatter, "expected `{expected}`, found `{found}`")
            }
            _ => formatter.write_str("type error"),
        }
    }
}

impl Error for TypeError {}
