//! EXPLORATORY — Elixir-flavoured surface syntax for Osiris.
//!
//! This module is a branch experiment, not a shipped feature. It translates
//! an Elixir-looking surface (`def`, `do … end`, `f(a, b)`, infix operators,
//! `|>`, keyword arguments) into canonical Osiris S-expression text. The
//! macro system is untouched: everything except `def`/`defmacro`, `@doc`,
//! and `if` is an ordinary call, so `defselect 名字 do … end` reaches the
//! very same named-body macros the S-expression surface uses.
//!
//! Deliberate boundaries of the sketch:
//! - identifiers use `_`, `-`, CJK, `?`, `!`; infix yields to kebab-case
//!   (OEP-0005) — `pct-rank` is one name, subtraction requires spaces
//!   (`a - b`)
//! - `|>` inserts the piped value as the FIRST argument (Elixir semantics)
//! - `quote`/`unquote` inside `defmacro` are not implemented; macro bodies
//!   are limited to plain phase-1 expressions
//! - no reader macros, no metadata other than `@doc`

/// One translation failure, positioned by 1-based source line.
#[derive(Debug)]
pub struct SketchError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for SketchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

/// Translate Elixir-flavoured surface text into Osiris S-expression text.
pub fn translate(source: &str) -> Result<String, Vec<SketchError>> {
    translate_with_lines(source).map(|(text, _)| text)
}

/// Translate and report, for every line of the OUTPUT, the 1-based source
/// line of the statement that produced it (0 when synthetic). Transitional
/// diagnostics mapping until the native reader lands (OEP-0005 R011).
pub fn translate_with_lines(source: &str) -> Result<(String, Vec<usize>), Vec<SketchError>> {
    let tokens = lex(source)?;
    let mut parser = Parser {
        tokens,
        position: 0,
    };
    let forms = parser.parse_program()?;
    Ok(render_program(&forms))
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    Number(String),
    Str(String),
    Atom(String),
    KeyArg(String),
    Operator(&'static str),
    Open(char),
    Close(char),
    Comma,
    Dot,
    Newline,
    AtDoc,
}

#[derive(Clone, Debug)]
struct Positioned {
    token: Token,
    line: usize,
}

fn lex(source: &str) -> Result<Vec<Positioned>, Vec<SketchError>> {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    let mut line = 1;
    let mut characters = source.chars().peekable();
    while let Some(&character) = characters.peek() {
        match character {
            '\n' => {
                characters.next();
                tokens.push(Positioned {
                    token: Token::Newline,
                    line,
                });
                line += 1;
            }
            character if character.is_whitespace() => {
                characters.next();
            }
            '#' => {
                while characters.peek().is_some_and(|&next| next != '\n') {
                    characters.next();
                }
            }
            '"' => {
                characters.next();
                let mut text = String::new();
                let mut closed = false;
                while let Some(next) = characters.next() {
                    match next {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' => match characters.next() {
                            Some('n') => text.push('\n'),
                            Some('t') => text.push('\t'),
                            Some(other) => text.push(other),
                            None => break,
                        },
                        '\n' => {
                            line += 1;
                            text.push('\n');
                        }
                        other => text.push(other),
                    }
                }
                if closed {
                    tokens.push(Positioned {
                        token: Token::Str(text),
                        line,
                    });
                } else {
                    errors.push(SketchError {
                        line,
                        message: "unterminated string".to_owned(),
                    });
                }
            }
            '(' | '[' | '{' => {
                characters.next();
                tokens.push(Positioned {
                    token: Token::Open(character),
                    line,
                });
            }
            ')' | ']' | '}' => {
                characters.next();
                tokens.push(Positioned {
                    token: Token::Close(character),
                    line,
                });
            }
            ',' => {
                characters.next();
                tokens.push(Positioned {
                    token: Token::Comma,
                    line,
                });
            }
            '.' => {
                characters.next();
                tokens.push(Positioned {
                    token: Token::Dot,
                    line,
                });
            }
            '@' => {
                characters.next();
                let word = lex_word(&mut characters);
                if word == "doc" {
                    tokens.push(Positioned {
                        token: Token::AtDoc,
                        line,
                    });
                } else {
                    errors.push(SketchError {
                        line,
                        message: format!("unknown attribute `@{word}`"),
                    });
                }
            }
            ':' => {
                characters.next();
                if characters.peek() == Some(&':') {
                    characters.next();
                    tokens.push(Positioned {
                        token: Token::Operator("::"),
                        line,
                    });
                } else {
                    let word = lex_word(&mut characters);
                    if word.is_empty() {
                        errors.push(SketchError {
                            line,
                            message: "expected atom name after `:`".to_owned(),
                        });
                    } else {
                        tokens.push(Positioned {
                            token: Token::Atom(word),
                            line,
                        });
                    }
                }
            }
            '`' => {
                characters.next();
                let mut word = String::new();
                let mut closed = false;
                for next in characters.by_ref() {
                    if next == '`' {
                        closed = true;
                        break;
                    }
                    if next == '\n' {
                        break;
                    }
                    word.push(next);
                }
                if closed && !word.is_empty() {
                    tokens.push(Positioned {
                        token: Token::Ident(word),
                        line,
                    });
                } else {
                    errors.push(SketchError {
                        line,
                        message: "expected a name between backticks".to_owned(),
                    });
                }
            }
            '|' => {
                characters.next();
                if characters.peek() == Some(&'>') {
                    characters.next();
                    tokens.push(Positioned {
                        token: Token::Operator("|>"),
                        line,
                    });
                } else {
                    errors.push(SketchError {
                        line,
                        message: "expected `|>`".to_owned(),
                    });
                }
            }
            '=' | '!' | '<' | '>' => {
                characters.next();
                let doubled = characters.peek() == Some(&'=');
                if doubled {
                    characters.next();
                }
                let operator = match (character, doubled) {
                    ('=', true) => "==",
                    ('!', true) => "!=",
                    ('<', true) => "<=",
                    ('>', true) => ">=",
                    ('<', false) => "<",
                    ('>', false) => ">",
                    _ => {
                        errors.push(SketchError {
                            line,
                            message: format!("unexpected `{character}`"),
                        });
                        continue;
                    }
                };
                tokens.push(Positioned {
                    token: Token::Operator(operator),
                    line,
                });
            }
            '+' | '-' | '*' | '/' => {
                characters.next();
                let operator = match character {
                    '+' => "+",
                    '-' => "-",
                    '*' => "*",
                    _ => "/",
                };
                tokens.push(Positioned {
                    token: Token::Operator(operator),
                    line,
                });
            }
            character if character.is_ascii_digit() => {
                let mut text = String::new();
                while characters.peek().is_some_and(|next| {
                    next.is_ascii_alphanumeric() || *next == '.' || *next == '_'
                }) {
                    // A trailing `.` belongs to a qualified name, not a number.
                    if *characters.peek().expect("peeked") == '.' {
                        let mut lookahead = characters.clone();
                        lookahead.next();
                        if !lookahead.peek().is_some_and(char::is_ascii_digit) {
                            break;
                        }
                    }
                    text.push(characters.next().expect("peeked"));
                }
                tokens.push(Positioned {
                    token: Token::Number(text),
                    line,
                });
            }
            character if is_ident_start(character) => {
                let word = lex_word(&mut characters);
                if characters.peek() == Some(&':') && {
                    let mut lookahead = characters.clone();
                    lookahead.next();
                    lookahead.peek() != Some(&':')
                } {
                    characters.next();
                    tokens.push(Positioned {
                        token: Token::KeyArg(word),
                        line,
                    });
                } else {
                    tokens.push(Positioned {
                        token: Token::Ident(word),
                        line,
                    });
                }
            }
            other => {
                characters.next();
                errors.push(SketchError {
                    line,
                    message: format!("unexpected character `{other}`"),
                });
            }
        }
    }
    if errors.is_empty() {
        Ok(tokens)
    } else {
        Err(errors)
    }
}

fn is_ident_start(character: char) -> bool {
    character == '_' || (character.is_alphanumeric() && !character.is_ascii_digit())
}

fn is_ident_continue(character: char) -> bool {
    character == '_' || character == '?' || character == '!' || character.is_alphanumeric()
}

fn lex_word(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut word = String::new();
    loop {
        if characters
            .peek()
            .is_some_and(|&next| is_ident_continue(next))
        {
            word.push(characters.next().expect("peeked"));
            continue;
        }
        // Infix yields to ecosystem names (OEP-0005 R004): `-` and `/` glue
        // into the identifier when a name character follows without a space —
        // `pct-rank` and `py/import` are single names; subtraction and
        // division are written `a - b` and `a / b`.
        if matches!(characters.peek(), Some('-' | '/')) {
            let mut lookahead = characters.clone();
            lookahead.next();
            if lookahead
                .peek()
                .is_some_and(|&next| is_ident_continue(next))
            {
                word.push(characters.next().expect("peeked"));
                continue;
            }
        }
        break;
    }
    word
}

/// The translated tree: canonical Osiris forms ready for rendering.
#[derive(Clone, Debug)]
enum Sx {
    Sym(String),
    Kw(String),
    Str(String),
    Num(String),
    List(Vec<Sx>),
    Vector(Vec<Sx>),
    /// `^{…} form` — metadata pairs attached to a form.
    Meta(Vec<(Sx, Sx)>, Box<Sx>),
    /// `{k v k v}` — an even, flat map literal.
    MapLit(Vec<Sx>),
    /// A statement with its 1-based source line, for the output line map.
    Stmt(Box<Sx>, usize),
}

struct Parser {
    tokens: Vec<Positioned>,
    position: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position).map(|entry| &entry.token)
    }

    fn peek_at(&self, offset: usize) -> Option<&Token> {
        self.tokens
            .get(self.position + offset)
            .map(|entry| &entry.token)
    }

    fn line(&self) -> usize {
        self.tokens
            .get(self.position.min(self.tokens.len().saturating_sub(1)))
            .map_or(1, |entry| entry.line)
    }

    fn advance(&mut self) -> Option<Token> {
        let token = self
            .tokens
            .get(self.position)
            .map(|entry| entry.token.clone());
        if token.is_some() {
            self.position += 1;
        }
        token
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Some(Token::Newline)) {
            self.position += 1;
        }
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, Vec<SketchError>> {
        Err(vec![SketchError {
            line: self.line(),
            message: message.into(),
        }])
    }

    fn expect_ident(&mut self, what: &str) -> Result<String, Vec<SketchError>> {
        match self.advance() {
            Some(Token::Ident(word)) => Ok(word),
            other => self.error(format!("expected {what}, found {other:?}")),
        }
    }

    fn expect(&mut self, token: &Token, what: &str) -> Result<(), Vec<SketchError>> {
        match self.advance() {
            Some(found) if &found == token => Ok(()),
            other => self.error(format!("expected {what}, found {other:?}")),
        }
    }

    fn at_keyword(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Ident(found)) if found == word)
    }

    fn parse_program(&mut self) -> Result<Vec<Sx>, Vec<SketchError>> {
        let mut forms = Vec::new();
        let mut pending_doc: Option<Sx> = None;
        loop {
            self.skip_newlines();
            if self.peek().is_none() {
                break;
            }
            if matches!(self.peek(), Some(Token::AtDoc)) {
                self.advance();
                pending_doc = Some(self.parse_doc_value()?);
                continue;
            }
            let line = self.line();
            let mut form = self.parse_statement()?;
            if let Some(doc) = pending_doc.take() {
                form = Sx::Meta(vec![(Sx::Kw("doc".to_owned()), doc)], Box::new(form));
            }
            forms.push(Sx::Stmt(Box::new(form), line));
        }
        Ok(forms)
    }

    /// `@doc "…"` or `@doc default: "…", zh-CN: "…"` — the keyword form
    /// builds the localized `:doc` map (OEP-0005 R005, revision 3).
    fn parse_doc_value(&mut self) -> Result<Sx, Vec<SketchError>> {
        if let Some(Token::KeyArg(_)) = self.peek() {
            let mut pairs = Vec::new();
            loop {
                let Some(Token::KeyArg(key)) = self.peek().cloned() else {
                    return self.error("expected `key: \"…\"` in @doc");
                };
                self.advance();
                let value = match self.advance() {
                    Some(Token::Str(text)) => Sx::Str(text),
                    other => {
                        return self.error(format!("expected string in @doc, found {other:?}"));
                    }
                };
                let key = if key == "default" {
                    Sx::Kw("default".to_owned())
                } else {
                    Sx::Str(key)
                };
                pairs.push(key);
                pairs.push(value);
                if matches!(self.peek(), Some(Token::Comma)) {
                    self.advance();
                } else {
                    break;
                }
            }
            return Ok(Sx::MapLit(pairs));
        }
        match self.advance() {
            Some(Token::Str(text)) => Ok(Sx::Str(text)),
            other => self.error(format!("expected string after @doc, found {other:?}")),
        }
    }

    /// One statement: a definition, a paren-less call, or a bare expression.
    fn parse_statement(&mut self) -> Result<Sx, Vec<SketchError>> {
        if self.at_keyword("def") || self.at_keyword("defmacro") {
            return self.parse_definition();
        }
        // A statement head followed by an argument (no operator between) is a
        // paren-less call: `slot short_mom, weight: rank_threshold`,
        // `where pct_rank(x) > floor`, `module a.b`, `import lib, refer: [x]`.
        if matches!(self.peek(), Some(Token::Ident(word))
                if !matches!(word.as_str(), "if" | "not" | "do" | "else" | "end"))
            && self.starts_parenless_call()
        {
            let head = self.expect_ident("statement head")?;
            let head = self.finish_qualified(head)?;
            // Core form names are hyphenated in canonical Osiris; the sketch
            // surface writes them with underscores like any identifier.
            let head = match head.as_str() {
                "import_for_syntax" => "import-for-syntax".to_owned(),
                "defn_for_syntax" => "defn-for-syntax".to_owned(),
                other_head => other_head.to_owned(),
            };
            let mut items = vec![Sx::Sym(head)];
            self.parse_call_arguments_into(&mut items, /* parenless */ true)?;
            if self.at_keyword("do") {
                self.parse_do_block_into(&mut items)?;
            }
            return Ok(Sx::List(items));
        }
        self.parse_expression(0)
    }

    /// True when the current ident begins a paren-less call rather than an
    /// expression: the next token starts an argument instead of an operator,
    /// call parenthesis, or end of statement.
    fn starts_parenless_call(&self) -> bool {
        let mut offset = 1;
        // Skip a qualified-name tail: `a.b.c`.
        while matches!(self.peek_at(offset), Some(Token::Dot))
            && matches!(self.peek_at(offset + 1), Some(Token::Ident(_)))
        {
            offset += 2;
        }
        match self.peek_at(offset) {
            Some(
                Token::Ident(_)
                | Token::Number(_)
                | Token::Str(_)
                | Token::Atom(_)
                | Token::KeyArg(_)
                | Token::Open('['),
            ) => true,
            Some(Token::Open('(')) => false,
            _ => false,
        }
    }

    fn finish_qualified(&mut self, first: String) -> Result<String, Vec<SketchError>> {
        let mut name = first;
        while matches!(self.peek(), Some(Token::Dot))
            && matches!(self.peek_at(1), Some(Token::Ident(_)))
        {
            self.advance();
            let next = self.expect_ident("name segment")?;
            name.push('.');
            name.push_str(&next);
        }
        Ok(name)
    }

    /// `def name(param :: Type, …) :: Ret do body end` →
    /// `(defn ^Ret name [^Type param …] body…)`; `defmacro` likewise without
    /// type annotations.
    fn parse_definition(&mut self) -> Result<Sx, Vec<SketchError>> {
        let keyword = self.expect_ident("definition keyword")?;
        let declaration = if keyword == "def" { "defn" } else { "defmacro" };
        let name = self.expect_ident("definition name")?;
        self.expect(&Token::Open('('), "`(` after definition name")?;
        let mut parameters = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), Some(Token::Close(')'))) {
                self.advance();
                break;
            }
            let parameter = self.expect_ident("parameter name")?;
            let mut form = Sx::Sym(parameter);
            if matches!(self.peek(), Some(Token::Operator("::"))) {
                self.advance();
                let ty = self.expect_ident("parameter type")?;
                form = Sx::Meta(
                    vec![(Sx::Sym(format!("^{ty}")), Sx::Sym(String::new()))],
                    Box::new(form),
                );
            }
            parameters.push(form);
            if matches!(self.peek(), Some(Token::Comma)) {
                self.advance();
            }
        }
        let mut name_form = Sx::Sym(name);
        if matches!(self.peek(), Some(Token::Operator("::"))) {
            self.advance();
            let ty = self.expect_ident("return type")?;
            name_form = Sx::Meta(
                vec![(Sx::Sym(format!("^{ty}")), Sx::Sym(String::new()))],
                Box::new(name_form),
            );
        }
        if !self.at_keyword("do") {
            return self.error("expected `do` to open the definition body");
        }
        let mut items = vec![
            Sx::Sym(declaration.to_owned()),
            name_form,
            Sx::Vector(parameters),
        ];
        self.parse_do_block_into(&mut items)?;
        Ok(Sx::List(items))
    }

    /// Consume `do … end`, appending each inner statement as one form.
    fn parse_do_block_into(&mut self, items: &mut Vec<Sx>) -> Result<(), Vec<SketchError>> {
        self.advance(); // `do`
        loop {
            self.skip_newlines();
            if self.at_keyword("end") {
                self.advance();
                return Ok(());
            }
            if self.peek().is_none() {
                return self.error("missing `end`");
            }
            let line = self.line();
            let statement = self.parse_statement()?;
            items.push(Sx::Stmt(Box::new(statement), line));
        }
    }

    /// Comma-separated call arguments; keyword arguments become `:key value`
    /// pairs. Paren-less argument lists stop at the line end or at `do`.
    fn parse_call_arguments_into(
        &mut self,
        items: &mut Vec<Sx>,
        parenless: bool,
    ) -> Result<(), Vec<SketchError>> {
        loop {
            if parenless {
                if matches!(self.peek(), Some(Token::Newline) | None) || self.at_keyword("do") {
                    return Ok(());
                }
            } else {
                self.skip_newlines();
                if matches!(self.peek(), Some(Token::Close(')'))) {
                    self.advance();
                    return Ok(());
                }
            }
            if let Some(Token::KeyArg(key)) = self.peek().cloned() {
                self.advance();
                items.push(Sx::Kw(key));
                items.push(self.parse_expression(0)?);
            } else {
                items.push(self.parse_expression(0)?);
            }
            if matches!(self.peek(), Some(Token::Comma)) {
                self.advance();
            } else if !parenless {
                self.skip_newlines();
                if matches!(self.peek(), Some(Token::Close(')'))) {
                    self.advance();
                    return Ok(());
                }
                return self.error("expected `,` or `)` in argument list");
            }
        }
    }

    /// Precedence-climbing expression parser. `|>` binds loosest and inserts
    /// the piped value as the callee's first argument.
    fn parse_expression(&mut self, minimum: u8) -> Result<Sx, Vec<SketchError>> {
        let mut left = self.parse_unary()?;
        loop {
            let Some(Token::Operator(operator)) = self.peek() else {
                break;
            };
            let operator = *operator;
            let Some(precedence) = binary_precedence(operator) else {
                break;
            };
            if precedence < minimum {
                break;
            }
            self.advance();
            if operator == "|>" {
                self.skip_newlines();
                let callee = self.parse_unary()?;
                left = pipe_into(left, callee).map_err(|message| {
                    vec![SketchError {
                        line: self.line(),
                        message,
                    }]
                })?;
            } else {
                let right = self.parse_expression(precedence + 1)?;
                left = Sx::List(vec![
                    Sx::Sym(osiris_operator(operator).to_owned()),
                    left,
                    right,
                ]);
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Sx, Vec<SketchError>> {
        if matches!(self.peek(), Some(Token::Operator("-"))) {
            self.advance();
            let operand = self.parse_unary()?;
            return Ok(Sx::List(vec![
                Sx::Sym("-".to_owned()),
                Sx::Num("0".to_owned()),
                operand,
            ]));
        }
        if self.at_keyword("not") {
            self.advance();
            let operand = self.parse_unary()?;
            return Ok(Sx::List(vec![Sx::Sym("not".to_owned()), operand]));
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Sx, Vec<SketchError>> {
        if self.at_keyword("if") {
            return self.parse_if();
        }
        let mut base = match self.advance() {
            Some(Token::Ident(word)) => {
                let name = self.finish_qualified(word)?;
                Sx::Sym(name)
            }
            Some(Token::Number(text)) => Sx::Num(text),
            Some(Token::Str(text)) => Sx::Str(text),
            Some(Token::Atom(word)) => Sx::Kw(word),
            Some(Token::Open('(')) => {
                self.skip_newlines();
                let inner = self.parse_expression(0)?;
                self.skip_newlines();
                self.expect(&Token::Close(')'), "`)`")?;
                inner
            }
            Some(Token::Open('[')) => {
                let mut items = Vec::new();
                loop {
                    self.skip_newlines();
                    if matches!(self.peek(), Some(Token::Close(']'))) {
                        self.advance();
                        break;
                    }
                    items.push(self.parse_expression(0)?);
                    if matches!(self.peek(), Some(Token::Comma)) {
                        self.advance();
                    }
                }
                return Ok(Sx::Vector(items));
            }
            other => return self.error(format!("unexpected token {other:?}")),
        };
        if let Sx::Sym(name) = &base
            && matches!(self.peek(), Some(Token::Open('(')))
        {
            self.advance();
            let mut items = vec![Sx::Sym(name.clone())];
            self.parse_call_arguments_into(&mut items, /* parenless */ false)?;
            base = Sx::List(items);
        }
        // Postfix member chains (OEP-0005 R008A): once the base is an
        // evaluated expression, `.name(args)` is the member call
        // `(.name base args…)` and bare `.name` the member access
        // `(.-name base)`. Plain name paths never reach here — their dots
        // were glued into the qualified symbol above.
        while matches!(self.peek(), Some(Token::Dot))
            && matches!(self.peek_at(1), Some(Token::Ident(_)))
        {
            self.advance();
            let member = self.expect_ident("member name")?;
            if matches!(self.peek(), Some(Token::Open('('))) {
                self.advance();
                let mut items = vec![Sx::Sym(format!(".{member}")), base];
                self.parse_call_arguments_into(&mut items, /* parenless */ false)?;
                base = Sx::List(items);
            } else {
                base = Sx::List(vec![Sx::Sym(format!(".-{member}")), base]);
            }
        }
        Ok(base)
    }

    /// `if condition do consequent else alternative end` → `(if c t f)`.
    fn parse_if(&mut self) -> Result<Sx, Vec<SketchError>> {
        self.advance(); // `if`
        let condition = self.parse_expression(0)?;
        if !self.at_keyword("do") {
            return self.error("expected `do` after if condition");
        }
        self.advance();
        self.skip_newlines();
        let consequent = self.parse_expression(0)?;
        self.skip_newlines();
        let alternative = if self.at_keyword("else") {
            self.advance();
            self.skip_newlines();
            let form = self.parse_expression(0)?;
            self.skip_newlines();
            Some(form)
        } else {
            None
        };
        if !self.at_keyword("end") {
            return self.error("expected `end` to close if");
        }
        self.advance();
        let mut items = vec![Sx::Sym("if".to_owned()), condition, consequent];
        if let Some(alternative) = alternative {
            items.push(alternative);
        }
        Ok(Sx::List(items))
    }
}

fn binary_precedence(operator: &str) -> Option<u8> {
    match operator {
        "|>" => Some(1),
        "==" | "!=" | "<" | "<=" | ">" | ">=" => Some(3),
        "+" | "-" => Some(4),
        "*" | "/" => Some(5),
        _ => None,
    }
}

fn osiris_operator(operator: &str) -> &str {
    match operator {
        "==" => "=",
        "!=" => "not=",
        other => other,
    }
}

/// `value |> f(a, b)` → `(f value a b)`; `value |> f` → `(f value)`.
fn pipe_into(value: Sx, callee: Sx) -> Result<Sx, String> {
    match callee {
        Sx::Sym(name) => Ok(Sx::List(vec![Sx::Sym(name), value])),
        Sx::List(items) => {
            let mut piped = Vec::with_capacity(items.len() + 1);
            let mut iterator = items.into_iter();
            let head = iterator
                .next()
                .ok_or_else(|| "cannot pipe into an empty call".to_owned())?;
            piped.push(head);
            piped.push(value);
            piped.extend(iterator);
            Ok(Sx::List(piped))
        }
        _ => Err("`|>` expects a call or function name on its right".to_owned()),
    }
}

fn render_program(forms: &[Sx]) -> (String, Vec<usize>) {
    let mut sink = Sink {
        text: String::new(),
        lines: Vec::new(),
        current: 0,
    };
    for (index, form) in forms.iter().enumerate() {
        if index > 0 {
            sink.newline();
        }
        render_top(form, &mut sink);
        sink.newline();
    }
    let Sink {
        text, mut lines, ..
    } = sink;
    lines.push(0);
    (text, lines)
}

/// Rendered text plus, per finished line, the source line that produced it.
struct Sink {
    text: String,
    lines: Vec<usize>,
    current: usize,
}

impl Sink {
    fn push_str(&mut self, text: &str) {
        self.text.push_str(text);
    }

    fn push(&mut self, character: char) {
        self.text.push(character);
    }

    fn newline(&mut self) {
        self.text.push('\n');
        self.lines.push(self.current);
    }
}

/// Definitions and do-block calls render with their body one clause per
/// line; short forms stay flat.
fn render_top(form: &Sx, sink: &mut Sink) {
    match form {
        Sx::Stmt(inner, line) => {
            sink.current = *line;
            render_top(inner, sink);
        }
        Sx::Meta(pairs, inner) => {
            let mut metadata = String::new();
            render_metadata(pairs, &mut metadata);
            sink.push_str(&metadata);
            sink.newline();
            render_top(inner, sink);
        }
        Sx::List(items)
            if items.len() > 3
                && (matches!(items.first(), Some(Sx::Sym(head)) if head == "defn" || head == "defmacro")
                    || items
                        .iter()
                        .skip(2)
                        .all(|item| matches!(item, Sx::List(_) | Sx::Stmt(_, _)))) =>
        {
            let (header, body) = match items.first() {
                Some(Sx::Sym(head)) if head == "defn" || head == "defmacro" => (3, &items[3..]),
                _ => (2, &items[2..]),
            };
            sink.push('(');
            for (index, item) in items[..header.min(items.len())].iter().enumerate() {
                if index > 0 {
                    sink.push(' ');
                }
                let mut flat = String::new();
                render_flat(item, &mut flat);
                sink.push_str(&flat);
            }
            for item in body {
                sink.newline();
                sink.push_str("  ");
                if let Sx::Stmt(inner, line) = item {
                    sink.current = *line;
                    let mut flat = String::new();
                    render_flat(inner, &mut flat);
                    sink.push_str(&flat);
                } else {
                    let mut flat = String::new();
                    render_flat(item, &mut flat);
                    sink.push_str(&flat);
                }
            }
            sink.push(')');
        }
        other => {
            let mut flat = String::new();
            render_flat(other, &mut flat);
            sink.push_str(&flat);
        }
    }
}

fn render_metadata(pairs: &[(Sx, Sx)], output: &mut String) {
    output.push_str("^{");
    for (index, (key, value)) in pairs.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        render_flat(key, output);
        output.push(' ');
        render_flat(value, output);
    }
    output.push('}');
}

fn render_flat(form: &Sx, output: &mut String) {
    match form {
        Sx::Sym(name) => output.push_str(name),
        Sx::Kw(name) => {
            output.push(':');
            output.push_str(name);
        }
        Sx::Num(text) => output.push_str(text),
        Sx::Str(text) => {
            output.push('"');
            for character in text.chars() {
                match character {
                    '"' => output.push_str("\\\""),
                    '\\' => output.push_str("\\\\"),
                    '\n' => output.push_str("\\n"),
                    other => output.push(other),
                }
            }
            output.push('"');
        }
        Sx::List(items) => {
            output.push('(');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(' ');
                }
                render_flat(item, output);
            }
            output.push(')');
        }
        Sx::Vector(items) => {
            output.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(' ');
                }
                render_flat(item, output);
            }
            output.push(']');
        }
        Sx::Stmt(inner, _) => render_flat(inner, output),
        Sx::MapLit(items) => {
            output.push('{');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(' ');
                }
                render_flat(item, output);
            }
            output.push('}');
        }
        Sx::Meta(pairs, inner) => {
            // Inline type marks render as `^Type name`; general metadata as
            // `^{…} form`.
            if pairs.len() == 1
                && matches!(&pairs[0].1, Sx::Sym(text) if text.is_empty())
                && matches!(&pairs[0].0, Sx::Sym(mark) if mark.starts_with('^'))
            {
                if let Sx::Sym(mark) = &pairs[0].0 {
                    output.push_str(mark);
                    output.push(' ');
                }
            } else {
                render_metadata(pairs, output);
                output.push(' ');
            }
            render_flat(inner, output);
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
