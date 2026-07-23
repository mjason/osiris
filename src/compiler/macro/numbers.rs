use super::*;

#[derive(Clone, Copy)]
enum Number {
    Integer(i128),
    Float(f64),
}

fn parse_number(form: &Form, span: Span) -> Result<Number, EvalError> {
    match &form.kind {
        FormKind::Integer(value) => value
            .parse::<i128>()
            .map(Number::Integer)
            .map_err(|_| EvalError::evaluation("integer is outside the phase-1 range", span)),
        FormKind::Float(value) => value
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(Number::Float)
            .ok_or_else(|| EvalError::evaluation("invalid finite phase-1 float", span)),
        _ => Err(EvalError::evaluation("expected a number", span)),
    }
}

pub(super) fn numeric_builtin(name: &str, forms: &[Form], span: Span) -> Result<Form, EvalError> {
    let numbers = forms
        .iter()
        .map(|form| parse_number(form, span))
        .collect::<Result<Vec<_>, _>>()?;
    if matches!(name, "inc" | "dec") && numbers.len() != 1 {
        return Err(EvalError::evaluation(
            format!("`{name}` expects one argument"),
            span,
        ));
    }
    if name == "/" {
        if numbers.is_empty() {
            return Err(EvalError::evaluation(
                "`/` expects at least one argument",
                span,
            ));
        }
        let mut values = numbers.iter().map(|number| match number {
            Number::Integer(value) => *value as f64,
            Number::Float(value) => *value,
        });
        let first = values.next().expect("checked above");
        let mut result = if numbers.len() == 1 {
            1.0 / first
        } else {
            first
        };
        if numbers.len() > 1 {
            for value in values {
                result /= value;
            }
        }
        return finite_float(result, span);
    }
    let has_float = numbers
        .iter()
        .any(|number| matches!(number, Number::Float(_)));
    if has_float {
        let values = numbers
            .iter()
            .map(|number| match number {
                Number::Integer(value) => *value as f64,
                Number::Float(value) => *value,
            })
            .collect::<Vec<_>>();
        let result = match name {
            "+" => values.iter().sum(),
            "*" => values.iter().product(),
            "-" if values.len() == 1 => -values[0],
            "-" if !values.is_empty() => values[1..].iter().fold(values[0], |a, b| a - b),
            "inc" => values[0] + 1.0,
            "dec" => values[0] - 1.0,
            _ => return Err(EvalError::evaluation("invalid numeric arity", span)),
        };
        return finite_float(result, span);
    }
    let values = numbers
        .into_iter()
        .map(|number| match number {
            Number::Integer(value) => value,
            Number::Float(_) => unreachable!(),
        })
        .collect::<Vec<_>>();
    let checked = match name {
        "+" => values
            .iter()
            .try_fold(0_i128, |left, right| left.checked_add(*right)),
        "*" => values
            .iter()
            .try_fold(1_i128, |left, right| left.checked_mul(*right)),
        "-" if values.len() == 1 => values[0].checked_neg(),
        "-" if !values.is_empty() => values[1..]
            .iter()
            .try_fold(values[0], |left, right| left.checked_sub(*right)),
        "inc" => values[0].checked_add(1),
        "dec" => values[0].checked_sub(1),
        _ => return Err(EvalError::evaluation("invalid numeric arity", span)),
    };
    checked
        .map(|value| Form::new(FormKind::Integer(value.to_string()), span))
        .ok_or_else(|| EvalError::evaluation("phase-1 integer arithmetic overflow", span))
}

pub(super) fn compare_builtin(name: &str, forms: &[Form], span: Span) -> Result<Form, EvalError> {
    if forms.len() < 2 {
        return Err(EvalError::evaluation(
            format!("`{name}` expects at least two arguments"),
            span,
        ));
    }
    let values = forms
        .iter()
        .map(|form| parse_number(form, span))
        .map(|result| {
            result.map(|number| match number {
                Number::Integer(value) => value as f64,
                Number::Float(value) => value,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let result = values.windows(2).all(|pair| match name {
        "<" => pair[0] < pair[1],
        "<=" => pair[0] <= pair[1],
        ">" => pair[0] > pair[1],
        ">=" => pair[0] >= pair[1],
        _ => unreachable!(),
    });
    Ok(boolean(result, span))
}

fn finite_float(value: f64, span: Span) -> Result<Form, EvalError> {
    if !value.is_finite() {
        return Err(EvalError::evaluation(
            "phase-1 float arithmetic produced a non-finite value",
            span,
        ));
    }
    let mut spelling = value.to_string();
    if !spelling.contains(['.', 'e', 'E']) {
        spelling.push_str(".0");
    }
    Ok(Form::new(FormKind::Float(spelling), span))
}

pub(super) fn form_to_usize(form: &Form, span: Span) -> Result<usize, EvalError> {
    let FormKind::Integer(value) = &form.kind else {
        return Err(EvalError::evaluation(
            "expected a non-negative integer",
            span,
        ));
    };
    value
        .parse::<usize>()
        .map_err(|_| EvalError::evaluation("expected a non-negative integer", span))
}

pub(super) fn form_to_string(form: &Form, span: Span) -> Result<String, EvalError> {
    match &form.kind {
        FormKind::String(value) => Ok(value.clone()),
        _ => Err(EvalError::evaluation("expected a string", span)),
    }
}

pub(super) fn form_name_or_string(form: &Form, span: Span) -> Result<String, EvalError> {
    match &form.kind {
        FormKind::String(value) => Ok(value.clone()),
        FormKind::Symbol(name) | FormKind::Keyword(name) => Ok(name.spelling.clone()),
        _ => Err(EvalError::evaluation(
            "expected a string, symbol, or keyword",
            span,
        )),
    }
}
