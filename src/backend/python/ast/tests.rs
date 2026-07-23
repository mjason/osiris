use std::{
    io::Write,
    process::{Command, Stdio},
};

use super::*;

fn name(value: &str) -> Expr {
    Expr::name(value)
}

fn integer(value: i128) -> Expr {
    Expr::Literal(Literal::Integer(value))
}

fn parse_with_python(source: &str) {
    let Ok(mut child) = Command::new("python3")
        .args(["-c", "import ast, sys; ast.parse(sys.stdin.read())"])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return;
    };
    child
        .stdin
        .take()
        .expect("Python stdin should be piped")
        .write_all(source.as_bytes())
        .expect("source should be writable to Python");
    let output = child.wait_with_output().expect("Python should finish");
    assert!(
        output.status.success(),
        "Python rejected generated source:\n{source}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn renders_complete_module_and_python_accepts_it() {
    let module = Module::new(vec![
        Stmt::Import(Import::From {
            module: Some("dataclasses".into()),
            names: vec![ImportAlias::new("dataclass")],
            level: 0,
        }),
        Stmt::Import(Import::Direct(vec![ImportAlias::renamed("numpy", "np")])),
        Stmt::ClassDef(ClassDef {
            name: "Quote".into(),
            bases: vec![],
            keywords: vec![],
            decorators: vec![name("dataclass")],
            body: vec![
                Stmt::AnnAssign(AnnAssign {
                    target: name("value"),
                    annotation: name("float"),
                    value: None,
                }),
                Stmt::AnnAssign(AnnAssign {
                    target: name("source"),
                    annotation: name("str"),
                    value: Some(Expr::string("市场\n")),
                }),
            ],
        }),
        Stmt::FunctionDef(Box::new(FunctionDef {
            name: "normalize".into(),
            parameters: Parameters {
                positional_only: vec![Parameter {
                    name: "values".into(),
                    annotation: Some(name("list")),
                    default: None,
                }],
                positional: vec![Parameter {
                    name: "scale".into(),
                    annotation: Some(name("float")),
                    default: Some(Expr::Literal(Literal::Float(1.0))),
                }],
                vararg: None,
                keyword_only: vec![Parameter {
                    name: "strict".into(),
                    annotation: Some(name("bool")),
                    default: Some(Expr::Literal(Literal::Bool(true))),
                }],
                kwarg: None,
            },
            returns: Some(name("float")),
            decorators: vec![],
            is_async: false,
            body: vec![Stmt::Try(Try {
                body: vec![Stmt::If(IfStmt {
                    test: name("strict"),
                    body: vec![Stmt::Return(Some(Expr::BinOp {
                        left: Box::new(Expr::Subscript {
                            value: Box::new(name("values")),
                            slice: Box::new(Expr::Slice {
                                lower: None,
                                upper: Some(Box::new(integer(1))),
                                step: None,
                            }),
                        }),
                        op: BinaryOp::Multiply,
                        right: Box::new(name("scale")),
                    }))],
                    orelse: vec![Stmt::Return(Some(name("values")))],
                })],
                handlers: vec![ExceptHandler {
                    exception_type: Some(name("TypeError")),
                    name: Some("error".into()),
                    body: vec![Stmt::Raise(Raise {
                        exception: Some(Expr::call(
                            name("ValueError"),
                            vec![CallArgument::Positional(Expr::string("invalid input"))],
                        )),
                        cause: Some(name("error")),
                    })],
                }],
                orelse: vec![],
                finalbody: vec![Stmt::Expr(Expr::call(name("cleanup"), vec![]))],
            })],
        })),
    ]);

    let source = module.to_source().expect("module should render");
    assert_eq!(
        source,
        concat!(
            "from dataclasses import dataclass\n",
            "import numpy as np\n",
            "\n\n",
            "@dataclass\n",
            "class Quote:\n",
            "    value: float\n",
            "    source: str = \"市场\\n\"\n",
            "\n\n",
            "def normalize(values: list, /, scale: float = 1.0, *, strict: bool = True) -> float:\n",
            "    try:\n",
            "        if strict:\n",
            "            return values[:1] * scale\n",
            "        else:\n",
            "            return values\n",
            "    except TypeError as error:\n",
            "        raise ValueError(\"invalid input\") from error\n",
            "    finally:\n",
            "        cleanup()\n",
        )
    );
    parse_with_python(&source);
}

#[test]
fn preserves_operator_tree_with_parentheses() {
    let expression = Expr::BinOp {
        left: Box::new(Expr::UnaryOp {
            op: UnaryOp::Negative,
            operand: Box::new(name("a")),
        }),
        op: BinaryOp::Power,
        right: Box::new(Expr::BinOp {
            left: Box::new(name("b")),
            op: BinaryOp::Subtract,
            right: Box::new(Expr::BinOp {
                left: Box::new(name("c")),
                op: BinaryOp::Subtract,
                right: Box::new(name("d")),
            }),
        }),
    };
    let source = Module::new(vec![Stmt::Expr(expression)])
        .to_source()
        .expect("expression should render");
    assert_eq!(source, "(-a) ** (b - (c - d))\n");
    parse_with_python(&source);
}

#[test]
fn parenthesizes_negative_literals_on_the_left_of_power() {
    let expression = Expr::BinOp {
        left: Box::new(Expr::Literal(Literal::Float(-2.0))),
        op: BinaryOp::Power,
        right: Box::new(integer(2)),
    };
    let source = Module::new(vec![Stmt::Expr(expression)])
        .to_source()
        .expect("expression should render");
    assert_eq!(source, "(-2.0) ** 2\n");
    parse_with_python(&source);
}

#[test]
fn renders_multidimensional_slices() {
    let expression = Expr::Subscript {
        value: Box::new(name("frame")),
        slice: Box::new(Expr::Tuple(vec![
            Expr::Slice {
                lower: None,
                upper: None,
                step: None,
            },
            integer(0),
        ])),
    };
    let source = Module::new(vec![Stmt::Expr(expression)])
        .to_source()
        .expect("slice should render");
    assert_eq!(source, "frame[:, 0]\n");
    parse_with_python(&source);
}

#[test]
fn escapes_text_bytes_and_integral_floats_stably() {
    let module = Module::new(vec![Stmt::Expr(Expr::Tuple(vec![
        Expr::string("quote=\" slash=\\ nul=\0 中文"),
        Expr::Literal(Literal::Bytes(vec![0, b'"', b'\\', 0xff])),
        Expr::Literal(Literal::Float(-0.0)),
        Expr::Literal(Literal::Float(f64::INFINITY)),
    ]))]);
    let first = module.to_source().expect("literals should render");
    let second = module.to_source().expect("literals should render again");
    assert_eq!(first, second);
    assert_eq!(
        first,
        "(\"quote=\\\" slash=\\\\ nul=\\x00 中文\", b\"\\x00\\\"\\\\\\xff\", -0.0, float(\"inf\"))\n"
    );
    parse_with_python(&first);
}

#[test]
fn rejects_structurally_invalid_nodes() {
    let invalid_assignment = Module::new(vec![Stmt::Assign(Assign {
        targets: vec![],
        value: integer(1),
    })]);
    assert_eq!(
        invalid_assignment.to_source().unwrap_err().to_string(),
        "an assignment must contain at least one target"
    );

    let invalid_lambda = Module::new(vec![Stmt::Expr(Expr::Lambda {
        parameters: Box::new(Parameters {
            positional: vec![Parameter {
                name: "value".into(),
                annotation: Some(name("int")),
                default: None,
            }],
            ..Parameters::default()
        }),
        body: Box::new(name("value")),
    })]);
    assert_eq!(
        invalid_lambda.to_source().unwrap_err().to_string(),
        "lambda parameters cannot contain annotations"
    );
}
