//! A small calculator for the launcher: type a sum, get the answer.
//!
//! Supports `+ - * / % ^`, parentheses, unary minus, a few functions and
//! the constants `pi` and `e`. `x` and `×` multiply, `÷` divides, and a
//! comma is accepted as a decimal separator inside a number.

use std::fmt;

/// Why an expression could not be evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalcError {
    Empty,
    /// An unexpected character at this byte offset.
    Unexpected(usize),
    UnknownName(String),
    DivisionByZero,
    /// The result is not a real number (e.g. `sqrt(-1)`) or overflowed.
    NotANumber,
    /// The expression ended too early or brackets do not match.
    Incomplete,
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CalcError::Empty => f.write_str("Type a sum"),
            CalcError::Unexpected(at) => write!(f, "Unexpected character at position {}", at + 1),
            CalcError::UnknownName(name) => write!(f, "\"{name}\" is not a known function"),
            CalcError::DivisionByZero => f.write_str("Cannot divide by zero"),
            CalcError::NotANumber => f.write_str("The result is not a real number"),
            CalcError::Incomplete => f.write_str("The sum is not finished"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Name(String),
    Op(char),
    Open,
    Close,
}

fn tokenize(input: &str) -> Result<Vec<Token>, CalcError> {
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let mut text = String::new();
            while i < chars.len() {
                let d = chars[i].1;
                let next_is_digit = chars.get(i + 1).is_some_and(|(_, n)| n.is_ascii_digit());
                if d.is_ascii_digit() || d == '.' {
                    text.push(d);
                } else if d == ',' && next_is_digit {
                    // "2,5" is two and a half.
                    text.push('.');
                } else if d == '_' && next_is_digit {
                    // "1_000" is a thousand.
                } else {
                    break;
                }
                i += 1;
            }
            let value: f64 = text.parse().map_err(|_| CalcError::Unexpected(at))?;
            tokens.push(Token::Number(value));
        } else if c.is_alphabetic() {
            let mut name = String::new();
            while i < chars.len() && chars[i].1.is_alphabetic() {
                name.push(chars[i].1.to_ascii_lowercase());
                i += 1;
            }
            // A lone "x" between operands means multiply.
            if name == "x" {
                tokens.push(Token::Op('*'));
            } else {
                tokens.push(Token::Name(name));
            }
        } else {
            let token = match c {
                '+' | '-' | '*' | '/' | '%' | '^' => Token::Op(c),
                '×' | '·' => Token::Op('*'),
                '÷' => Token::Op('/'),
                '−' => Token::Op('-'),
                '(' => Token::Open,
                ')' => Token::Close,
                _ => return Err(CalcError::Unexpected(at)),
            };
            tokens.push(token);
            i += 1;
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        token
    }

    // expr := term (('+' | '-') term)*
    fn expr(&mut self) -> Result<f64, CalcError> {
        let mut value = self.term()?;
        while let Some(Token::Op(op @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let rhs = self.term()?;
            value = if op == '+' { value + rhs } else { value - rhs };
        }
        Ok(value)
    }

    // term := unary (('*' | '/' | '%') unary | implicit-multiply)*
    fn term(&mut self) -> Result<f64, CalcError> {
        let mut value = self.unary()?;
        loop {
            match self.peek().cloned() {
                Some(Token::Op(op @ ('*' | '/' | '%'))) => {
                    self.pos += 1;
                    let rhs = self.unary()?;
                    value = match op {
                        '*' => value * rhs,
                        _ if rhs == 0.0 => return Err(CalcError::DivisionByZero),
                        '/' => value / rhs,
                        _ => value % rhs,
                    };
                }
                // "2(3+4)" and "2pi" multiply without an operator.
                Some(Token::Open) | Some(Token::Name(_)) => {
                    let rhs = self.power()?;
                    value *= rhs;
                }
                _ => return Ok(value),
            }
        }
    }

    // unary := '-' unary | '+' unary | power
    fn unary(&mut self) -> Result<f64, CalcError> {
        match self.peek() {
            Some(Token::Op('-')) => {
                self.pos += 1;
                Ok(-self.unary()?)
            }
            Some(Token::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    // power := atom ('^' unary)?   (right associative)
    fn power(&mut self) -> Result<f64, CalcError> {
        let base = self.atom()?;
        if let Some(Token::Op('^')) = self.peek() {
            self.pos += 1;
            let exponent = self.unary()?;
            return Ok(base.powf(exponent));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<f64, CalcError> {
        match self.next() {
            Some(Token::Number(n)) => Ok(n),
            Some(Token::Open) => {
                let value = self.expr()?;
                match self.next() {
                    Some(Token::Close) => Ok(value),
                    _ => Err(CalcError::Incomplete),
                }
            }
            Some(Token::Name(name)) => match name.as_str() {
                "pi" | "π" => Ok(std::f64::consts::PI),
                "e" => Ok(std::f64::consts::E),
                "tau" => Ok(std::f64::consts::TAU),
                _ => {
                    let function: fn(f64) -> f64 = match name.as_str() {
                        "sqrt" => f64::sqrt,
                        "abs" => f64::abs,
                        "sin" => f64::sin,
                        "cos" => f64::cos,
                        "tan" => f64::tan,
                        "ln" => f64::ln,
                        "log" => f64::log10,
                        "round" => f64::round,
                        "floor" => f64::floor,
                        "ceil" => f64::ceil,
                        _ => return Err(CalcError::UnknownName(name)),
                    };
                    match self.next() {
                        Some(Token::Open) => {}
                        _ => return Err(CalcError::Incomplete),
                    }
                    let argument = self.expr()?;
                    match self.next() {
                        Some(Token::Close) => Ok(function(argument)),
                        _ => Err(CalcError::Incomplete),
                    }
                }
            },
            Some(Token::Close) | Some(Token::Op(_)) | None => Err(CalcError::Incomplete),
        }
    }
}

/// Evaluates an arithmetic expression.
pub fn evaluate(input: &str) -> Result<f64, CalcError> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(CalcError::Empty);
    }
    let mut parser = Parser { tokens, pos: 0 };
    let value = parser.expr()?;
    if parser.pos < parser.tokens.len() {
        return Err(CalcError::Incomplete);
    }
    if !value.is_finite() {
        return Err(CalcError::NotANumber);
    }
    Ok(value)
}

/// Formats a result the way a person would write it: no trailing zeros,
/// no floating point noise, and no `-0`.
pub fn format(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let magnitude = value.abs();
    if !(1e-6..1e15).contains(&magnitude) {
        let text = format!("{value:.6e}");
        // Trim zeros in the mantissa: 1.500000e20 becomes 1.5e20.
        if let Some((mantissa, exponent)) = text.split_once('e') {
            let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
            return format!("{mantissa}e{exponent}");
        }
        return text;
    }
    // Ten significant decimals hides errors like 0.1 + 0.2.
    let text = format!("{value:.10}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.to_string()
    }
}

/// Whether a query should be treated as a sum without the `=` prefix:
/// it must contain a digit and something to calculate, and it must
/// evaluate cleanly.
pub fn looks_like_math(input: &str) -> bool {
    let has_digit = input.chars().any(|c| c.is_ascii_digit());
    let has_operation = input.chars().any(|c| "+-*/^%×÷(".contains(c))
        || ["sqrt", "sin", "cos", "tan", "ln", "log", "abs"]
            .iter()
            .any(|f| input.contains(f));
    // A bare number, or text such as "3d-printer", is not a sum.
    let starts_like_a_sum = input
        .trim_start()
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || "-(.".contains(c) || c.is_alphabetic());
    has_digit && has_operation && starts_like_a_sum && evaluate(input).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(s: &str) -> String {
        format(evaluate(s).unwrap())
    }

    #[test]
    fn basic_arithmetic_and_precedence() {
        assert_eq!(eval("2+2"), "4");
        assert_eq!(eval("2 + 3 * 4"), "14");
        assert_eq!(eval("(2 + 3) * 4"), "20");
        assert_eq!(eval("10 / 4"), "2.5");
        assert_eq!(eval("10 % 3"), "1");
        assert_eq!(eval("7 - 10"), "-3");
    }

    #[test]
    fn powers_are_right_associative_and_bind_tighter_than_negation() {
        assert_eq!(eval("2^10"), "1024");
        assert_eq!(eval("2^3^2"), "512");
        assert_eq!(eval("-2^2"), "-4");
        assert_eq!(eval("2^-1"), "0.5");
    }

    #[test]
    fn unary_signs() {
        assert_eq!(eval("-5 + 3"), "-2");
        assert_eq!(eval("3 - -2"), "5");
        assert_eq!(eval("+4"), "4");
    }

    #[test]
    fn friendly_notation() {
        assert_eq!(eval("3 x 4"), "12");
        assert_eq!(eval("3 × 4"), "12");
        assert_eq!(eval("12 ÷ 4"), "3");
        assert_eq!(eval("2,5 * 2"), "5");
        assert_eq!(eval("1_000 + 1"), "1001");
        assert_eq!(eval("2(3+4)"), "14");
        assert_eq!(eval("2pi"), "6.2831853072");
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(eval("sqrt(16)"), "4");
        assert_eq!(eval("abs(-3)"), "3");
        assert_eq!(eval("round(2.6)"), "3");
        assert_eq!(eval("log(1000)"), "3");
        assert_eq!(eval("ln(e)"), "1");
        assert_eq!(eval("cos(0)"), "1");
        assert_eq!(eval("SQRT(9)"), "3");
    }

    #[test]
    fn hides_floating_point_noise() {
        assert_eq!(eval("0.1 + 0.2"), "0.3");
        assert_eq!(eval("1/3"), "0.3333333333");
        assert_eq!(eval("0 * -1"), "0");
    }

    #[test]
    fn very_large_and_small_results_use_exponents() {
        assert_eq!(eval("10^20"), "1e20");
        assert_eq!(eval("1.5 * 10^20"), "1.5e20");
        assert_eq!(eval("1 / 10^9"), "1e-9");
    }

    #[test]
    fn errors_are_specific() {
        assert_eq!(evaluate(""), Err(CalcError::Empty));
        assert_eq!(evaluate("   "), Err(CalcError::Empty));
        assert_eq!(evaluate("1/0"), Err(CalcError::DivisionByZero));
        assert_eq!(evaluate("5 % 0"), Err(CalcError::DivisionByZero));
        assert_eq!(evaluate("2 +"), Err(CalcError::Incomplete));
        assert_eq!(evaluate("(2 + 3"), Err(CalcError::Incomplete));
        assert_eq!(evaluate("2 + 3)"), Err(CalcError::Incomplete));
        assert_eq!(evaluate("2 $ 3"), Err(CalcError::Unexpected(2)));
        assert_eq!(
            evaluate("foo(2)"),
            Err(CalcError::UnknownName("foo".into()))
        );
        assert_eq!(evaluate("sqrt(-1)"), Err(CalcError::NotANumber));
        assert_eq!(evaluate("sqrt 4"), Err(CalcError::Incomplete));
    }

    #[test]
    fn recognises_sums_but_not_ordinary_text() {
        for yes in [
            "2+2",
            "12 * (3 + 4)",
            "sqrt(16)",
            "-5 + 3",
            "2^8",
            "(1+2)*3",
        ] {
            assert!(looks_like_math(yes), "{yes} should be a sum");
        }
        for no in [
            "firefox",
            "42",
            "3d-printer",
            "wi-fi",
            "c++",
            "2 +",
            "vim 9",
            "+1 reaction",
        ] {
            assert!(!looks_like_math(no), "{no} should not be a sum");
        }
    }
}
