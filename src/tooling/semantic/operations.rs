use super::*;

pub(super) fn build_operation_graph(
    module: &hir::Module,
    traces: &[ExpansionTrace],
) -> OperationGraph {
    let aliases = aliases_by_target(module);
    let mut builder = OperationBuilder {
        next: 0,
        nodes: Vec::new(),
        edges: Vec::new(),
        traces,
        bindings: module
            .bindings
            .iter()
            .map(|binding| {
                let id = binding.name.id.as_str().to_owned();
                let binding_aliases = aliases.get(&id).map(Vec::as_slice).unwrap_or_default();
                let label = labels_for_name(
                    &binding.name.canonical,
                    preferred_alias(binding_aliases, &binding.metadata),
                );
                (id, (binding.name.canonical.clone(), label))
            })
            .collect(),
    };
    for item in &module.items {
        builder.item(item);
    }
    let mut outputs = BTreeMap::<String, Vec<String>>::new();
    for edge in &builder.edges {
        outputs
            .entry(edge.from.clone())
            .or_default()
            .push(edge.to.clone());
    }
    for node in &mut builder.nodes {
        node.outputs = outputs.remove(&node.id).unwrap_or_default();
        node.outputs.sort();
        node.outputs.dedup();
    }
    OperationGraph {
        nodes: builder.nodes,
        edges: builder.edges,
    }
}

struct OperationBuilder<'a> {
    next: usize,
    nodes: Vec<OperationNode>,
    edges: Vec<OperationEdge>,
    traces: &'a [ExpansionTrace],
    bindings: BTreeMap<String, (String, LocalizedLabel)>,
}

impl OperationBuilder<'_> {
    fn id(&mut self) -> String {
        let id = format!("op-{}", self.next);
        self.next += 1;
        id
    }

    fn add(
        &mut self,
        kind: impl Into<String>,
        span: Span,
        binding_id: Option<String>,
        ty: Type,
        summaries: &CallSummaries,
        inputs: Vec<String>,
    ) -> String {
        let raw_kind = kind.into();
        let id = self.id();
        let binding_label = binding_id
            .as_ref()
            .and_then(|binding| self.bindings.get(binding))
            .cloned();
        let labels = binding_label
            .map(|(_, labels)| labels)
            .unwrap_or_else(|| operation_labels(&raw_kind));
        let macro_origins = self
            .traces
            .iter()
            .filter(|trace| {
                spans_overlap(trace.call_span, span) || spans_overlap(trace.expansion_span, span)
            })
            .flat_map(|trace| trace.origin.iter().map(|origin| (origin.start, origin.end)))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|(start, end)| Span::new(start, end))
            .collect::<Vec<_>>();
        for input in &inputs {
            self.edges.push(OperationEdge {
                from: input.clone(),
                to: id.clone(),
                kind: "data".to_owned(),
            });
        }
        self.nodes.push(OperationNode {
            id: id.clone(),
            kind: raw_kind,
            span,
            binding_id,
            ty,
            summary: SemanticSummary::from_call(summaries),
            labels,
            inputs,
            outputs: Vec::new(),
            macro_origins,
        });
        id
    }

    fn item(&mut self, item: &hir::Item) {
        match &item.kind {
            ItemKind::Import(import) => {
                self.add(
                    "import",
                    item.span,
                    Some(import.binding.as_str().to_owned()),
                    Type::Any,
                    &CallSummaries::pure_scalar(),
                    Vec::new(),
                );
            }
            ItemKind::Value(value) => {
                let input = value.value.as_ref().map(|expr| self.expr(expr));
                let ty = value
                    .value
                    .as_ref()
                    .map_or(Type::Any, |expr| expr.ty.clone());
                let summary = value
                    .value
                    .as_ref()
                    .map_or_else(CallSummaries::pure_scalar, |expr| expr.summaries.clone());
                self.add(
                    "value",
                    item.span,
                    Some(value.binding.as_str().to_owned()),
                    ty,
                    &summary,
                    input.into_iter().collect(),
                );
            }
            ItemKind::Function(function) => {
                let mut inputs = function
                    .decorators
                    .iter()
                    .map(|decorator| self.expr(decorator))
                    .collect::<Vec<_>>();
                let body = self.expr(&function.body);
                inputs.push(body);
                let ty = Type::Fn(
                    crate::types::FunctionType::new(
                        function
                            .parameters
                            .iter()
                            .map(|parameter| parameter.ty.clone())
                            .collect(),
                        function.return_type.clone(),
                    )
                    .with_summaries(function.summaries.clone()),
                );
                self.add(
                    "function",
                    item.span,
                    Some(function.binding.as_str().to_owned()),
                    ty,
                    &function.summaries,
                    inputs,
                );
            }
            ItemKind::Struct(structure) => {
                let mut inputs = structure
                    .decorators
                    .iter()
                    .map(|decorator| self.expr(decorator))
                    .collect::<Vec<_>>();
                inputs.extend(
                    structure.fields.iter().filter_map(|field| {
                        field.default.as_ref().map(|default| self.expr(default))
                    }),
                );
                self.add(
                    "struct",
                    item.span,
                    Some(structure.binding.as_str().to_owned()),
                    Type::Any,
                    &CallSummaries::pure_scalar(),
                    inputs,
                );
            }
            ItemKind::Expr(expression) => {
                self.expr(expression);
            }
            ItemKind::StaticSchema(schema) => {
                self.add(
                    "static-schema",
                    schema.span,
                    None,
                    Type::Any,
                    &CallSummaries::pure_scalar(),
                    Vec::new(),
                );
            }
            ItemKind::StaticRecord(record) => {
                self.add(
                    "static-record",
                    record.span,
                    None,
                    Type::Any,
                    &CallSummaries::pure_scalar(),
                    Vec::new(),
                );
            }
        }
    }

    fn expr(&mut self, expression: &Expr) -> String {
        let mut inputs = Vec::new();
        let mut binding_id = None;
        let kind = match &expression.kind {
            ExprKind::Binding(binding) => {
                return self.add(
                    "binding",
                    expression.span,
                    Some(binding.as_str().to_owned()),
                    expression.ty.clone(),
                    &expression.summaries,
                    Vec::new(),
                );
            }
            ExprKind::Call { callee, arguments } => {
                if let ExprKind::Binding(binding) = &callee.kind {
                    binding_id = Some(binding.as_str().to_owned());
                }
                inputs.push(self.expr(callee));
                for argument in arguments {
                    inputs.push(match argument {
                        hir::CallArgument::Positional(value)
                        | hir::CallArgument::Keyword { value, .. } => self.expr(value),
                    });
                }
                "call"
            }
            ExprKind::Operator { operator, operands } => {
                inputs.extend(operands.iter().map(|operand| self.expr(operand)));
                return self.add(
                    format!("operator:{operator:?}").to_ascii_lowercase(),
                    expression.span,
                    None,
                    expression.ty.clone(),
                    &expression.summaries,
                    inputs,
                );
            }
            ExprKind::Attribute { value, attribute } => {
                inputs.push(self.expr(value));
                return self.add(
                    format!("attribute:{attribute}"),
                    expression.span,
                    None,
                    expression.ty.clone(),
                    &expression.summaries,
                    inputs,
                );
            }
            ExprKind::Index { value, index } => {
                inputs.push(self.expr(value));
                inputs.push(self.expr(index));
                "index"
            }
            ExprKind::List(items) => {
                inputs.extend(items.iter().map(|item| self.expr(item)));
                "list"
            }
            ExprKind::Vector(items) => {
                inputs.extend(items.iter().map(|item| self.expr(item)));
                "vector"
            }
            ExprKind::Set(items) => {
                inputs.extend(items.iter().map(|item| self.expr(item)));
                "set"
            }
            ExprKind::Map(entries) => {
                for (key, value) in entries {
                    inputs.push(self.expr(key));
                    inputs.push(self.expr(value));
                }
                "map"
            }
            ExprKind::Let { bindings, body } => {
                inputs.extend(bindings.iter().map(|binding| self.expr(&binding.value)));
                inputs.push(self.expr(body));
                "let"
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                inputs.push(self.expr(condition));
                inputs.push(self.expr(then_branch));
                inputs.push(self.expr(else_branch));
                "if"
            }
            ExprKind::Do(items) => {
                inputs.extend(items.iter().map(|item| self.expr(item)));
                "do"
            }
            ExprKind::Lambda { body, .. } => {
                inputs.push(self.expr(body));
                "lambda"
            }
            ExprKind::Try {
                body,
                catches,
                finally_body,
            } => {
                inputs.push(self.expr(body));
                inputs.extend(catches.iter().map(|catch| self.expr(&catch.body)));
                if let Some(finally_body) = finally_body {
                    inputs.push(self.expr(finally_body));
                }
                "try"
            }
            ExprKind::Raise(value) => {
                if let Some(value) = value {
                    inputs.push(self.expr(value));
                }
                "raise"
            }
            ExprKind::None => "none",
            ExprKind::Bool(_) => "bool",
            ExprKind::Integer(_) => "integer",
            ExprKind::Float(_) => "float",
            ExprKind::String(_) => "string",
            ExprKind::Error => "error",
        };
        self.add(
            kind,
            expression.span,
            binding_id,
            expression.ty.clone(),
            &expression.summaries,
            inputs,
        )
    }
}

pub(super) fn spans_overlap(left: Span, right: Span) -> bool {
    left.start <= right.end && right.start <= left.end
}

pub(super) fn operation_labels(kind: &str) -> LocalizedLabel {
    let zh_cn = match kind.split_once(':').map_or(kind, |(head, _)| head) {
        "import" => "导入",
        "value" => "值定义",
        "function" => "函数",
        "struct" => "结构",
        "static-schema" => "静态模式",
        "static-record" => "静态记录",
        "binding" => "绑定",
        "call" => "调用",
        "operator" => "运算",
        "attribute" => "字段访问",
        "index" => "索引",
        "list" => "列表",
        "vector" => "向量",
        "set" => "集合",
        "map" => "映射",
        "let" => "局部绑定",
        "if" => "条件",
        "do" => "顺序执行",
        "lambda" => "匿名函数",
        "try" => "异常处理",
        "raise" => "抛出异常",
        "none" => "空值",
        "bool" => "布尔值",
        "integer" => "整数",
        "float" => "浮点数",
        "string" => "字符串",
        "error" => "错误",
        _ => kind,
    };
    LocalizedLabel {
        zh_cn: zh_cn.to_owned(),
        en: kind.to_owned(),
    }
}
