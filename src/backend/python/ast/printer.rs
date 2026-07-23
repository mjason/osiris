use super::*;

/// Render a module as Python 3.9 source.
pub fn render(module: &Module) -> Result<String, PrintError> {
    let mut printer = Printer::default();
    printer.print_module(module)?;
    Ok(printer.output)
}

const PREC_LAMBDA: u8 = 1;
const PREC_IF_EXP: u8 = 2;
const PREC_OR: u8 = 3;
const PREC_AND: u8 = 4;
const PREC_NOT: u8 = 5;
const PREC_COMPARE: u8 = 6;
const PREC_BIT_OR: u8 = 7;
const PREC_BIT_XOR: u8 = 8;
const PREC_BIT_AND: u8 = 9;
const PREC_SHIFT: u8 = 10;
const PREC_ADD: u8 = 11;
const PREC_MULTIPLY: u8 = 12;
const PREC_UNARY: u8 = 13;
const PREC_POWER: u8 = 14;
const PREC_PRIMARY: u8 = 16;
const PREC_ATOM: u8 = 17;

#[derive(Default)]
struct Printer {
    output: String,
    indent: usize,
}

mod expressions;
mod statements;
mod validation;

use validation::*;
