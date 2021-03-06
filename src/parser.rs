use crate::symbols::{Expr, LispResult, Num, ProgramError};

// s-expression parser using nom.
// Supports the usual constructs (quotes, numbers, strings, comments)

// HUGE thanks to the nom people (Geal, adamnemecek, MarcMcCaskey, et all)
// who had an s_expression example for me to work from.
// https://github.com/Geal/nom/blob/master/examples/s_expression.rs

use nom::bytes::complete::escaped;
use nom::{
    branch::alt,
    bytes::complete::tag,
    bytes::complete::{take_till, take_while1},
    character::complete::{char, multispace0, none_of},
    combinator::{cut, map, map_res},
    error::{context, VerboseError},
    multi::many0,
    number::complete::recognize_float,
    sequence::{delimited, preceded},
    IResult, Parser,
};

#[inline]
fn is_symbol_char(c: char) -> bool {
    match c {
        '(' | ')' => false,
        '"' => false,
        '\'' => false,
        ';' => false,
        ' ' => false,
        sym => !sym.is_whitespace(),
    }
}

use crate::symbols::SymbolTable;
use im::Vector;

fn method_call(method: String) -> Expr {
    use crate::symbols::Function;
    use std::sync::Arc;
    let method_clone = method.clone();
    let method_fn = move |args: Vector<Expr>, _sym: &SymbolTable| {
        let rec = match args[0].get_record() {
            Ok(rec) => rec,
            Err(e) => return Err(e),
        };
        use crate::records::Record;
        rec.call_method(&method_clone, args.clone().slice(1..))
    };
    let f = Function::new(
        format!("method_call<{}>", method),
        1,
        Arc::new(method_fn),
        true,
    );
    Expr::Function(f)
}

fn parse_symbol<'a>(i: &'a str) -> IResult<&'a str, Expr, VerboseError<&'a str>> {
    map(take_while1(is_symbol_char), |sym: &str| {
        if sym.starts_with('.') {
            method_call(sym[1..].into())
        } else {
            Expr::Symbol(sym.into())
        }
    })(i)
}

fn parse_string<'a>(i: &'a str) -> IResult<&'a str, Expr, VerboseError<&'a str>> {
    let esc = escaped(none_of("\\\""), '\\', tag("\""));
    let esc_or_empty = alt((esc, tag("")));

    map(delimited(tag("\""), esc_or_empty, tag("\"")), |s: &str| {
        Expr::String(s.into())
    })(i)
}

fn parse_bool<'a>(i: &'a str) -> IResult<&'a str, Expr, VerboseError<&'a str>> {
    alt((
        map(tag("true"), |_| Expr::Bool(true)),
        map(tag("false"), |_| Expr::Bool(false)),
    ))(i)
}

fn ignored_input<'a>(i: &'a str) -> IResult<&'a str, &'a str, VerboseError<&'a str>> {
    let comment_parse = delimited(
        preceded(multispace0, tag(";")),
        take_till(|c| c == '\n'),
        multispace0,
    );
    alt((comment_parse, multispace0))(i)
}

fn parse_tuple<'a>(i: &'a str) -> IResult<&'a str, Expr, VerboseError<&'a str>> {
    let make_tuple = |exprs: Vec<_>| {
        let mut tuple_list = im::vector![Expr::Symbol("tuple".into())];
        tuple_list.append(exprs.into());
        Expr::List(tuple_list)
    };
    map(
        context("quote", preceded(tag("^"), cut(s_exp(many0(parse_expr))))),
        make_tuple,
    )(i)
}

fn parse_quote<'a>(i: &'a str) -> IResult<&'a str, Expr, VerboseError<&'a str>> {
    map(
        context("quote", preceded(tag("'"), cut(s_exp(many0(parse_expr))))),
        |exprs| Expr::Quote(exprs.into()),
    )(i)
}

fn parse_num<'a>(i: &'a str) -> IResult<&'a str, Expr, VerboseError<&'a str>> {
    map_res(recognize_float, |digit_str: &str| {
        digit_str.parse::<Num>().map(Expr::Num)
    })(i)
}

fn s_exp<'a, O1, F>(inner: F) -> impl FnMut(&'a str) -> IResult<&'a str, O1, VerboseError<&'a str>>
where
    F: Parser<&'a str, O1, VerboseError<&'a str>>,
{
    delimited(
        char('('),
        preceded(ignored_input, inner),
        context("closing paren", cut(preceded(ignored_input, char(')')))),
    )
}

fn parse_list<'a>(i: &'a str) -> IResult<&'a str, Expr, VerboseError<&'a str>> {
    let application_inner = map(many0(parse_expr), |l| Expr::List(l.into()));
    // finally, we wrap it in an s-expression
    s_exp(application_inner)(i)
}

fn parse_expr<'a>(i: &'a str) -> IResult<&'a str, Expr, VerboseError<&'a str>> {
    delimited(
        ignored_input,
        alt((
            parse_list,
            parse_quote,
            parse_tuple,
            parse_string,
            parse_num,
            parse_bool,
            parse_symbol,
        )),
        ignored_input,
    )(i)
}

pub(crate) struct ExprIterator<'a> {
    input: &'a str,
    done: bool,
}

impl<'a> ExprIterator<'a> {
    pub(crate) fn new(input: &'a str) -> Self {
        Self { input, done: false }
    }
}

impl<'a> Iterator for ExprIterator<'a> {
    type Item = LispResult<Expr>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.input.is_empty() {
            return None;
        }
        let (rest, res) = match parse_expr(self.input) {
            Ok(r) => r,
            Err(e) => {
                self.done = true;
                return Some(Err(anyhow::Error::new(ProgramError::FailedToParse(
                    e.to_string(),
                ))));
            }
        };
        self.input = rest;
        Some(Ok(res))
    }
}

pub(crate) fn read(s: &str) -> ExprIterator {
    ExprIterator::new(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigdecimal::{BigDecimal, FromPrimitive};

    macro_rules! num_f {
        ($n:expr) => {
            Expr::Num(BigDecimal::from_f64($n).unwrap())
        };
    }

    #[test]
    fn parse_floats() {
        assert_eq!(parse_num("1").unwrap(), ("", num_f!(1.0)));
        assert_eq!(parse_num("1.0").unwrap(), ("", num_f!(1.0)));
        assert_eq!(parse_num("1.1").unwrap(), ("", num_f!(1.1)));
        assert_eq!(parse_num("-1.1").unwrap(), ("", num_f!(-1.1)));
        assert_eq!(parse_num("-0.1").unwrap(), ("", num_f!(-0.1)));
    }

    macro_rules! test_symbol {
        ($($sym:literal),*) => {
            $(
                assert_eq!(
                    parse_symbol($sym).unwrap(),
                    ("", Expr::Symbol($sym.into()))
                );
            )*
        }
    }

    #[test]
    fn parse_sym() {
        test_symbol!("abc", "abc1", "empty?", "test", "foo-bar", "-foobar");
    }

    // TODO: Make this way less brittle
    #[test]
    fn parse_str() {
        assert_eq!(
            parse_string(r#""1""#).unwrap(),
            ("", Expr::String("1".into()))
        );

        assert_eq!(
            parse_string(r#""""#).unwrap(),
            ("", Expr::String("".into()))
        );

        assert_eq!(
            parse_string(r#""hello-world""#).unwrap(),
            ("", Expr::String("hello-world".into()))
        );

        assert_eq!(
            parse_string(r#""hello world""#).unwrap(),
            ("", Expr::String("hello world".into()))
        );

        assert_eq!(
            parse_string(r#""hello? world""#).unwrap(),
            ("", Expr::String("hello? world".into()))
        );
    }

    #[test]
    fn parse_ex() {
        assert_eq!(parse_expr("1").unwrap(), ("", num_f!(1.0)));
        assert_eq!(
            parse_expr(r#""hello? world""#).unwrap(),
            ("", Expr::String("hello? world".into()))
        );
        assert_eq!(
            parse_expr(r#""1""#).unwrap(),
            ("", Expr::String("1".into()))
        );

        assert_eq!(parse_expr(r#""""#).unwrap(), ("", Expr::String("".into())));

        assert_eq!(
            parse_expr(r#""hello-world""#).unwrap(),
            ("", Expr::String("hello-world".into()))
        );

        assert_eq!(
            parse_expr(r#""hello world""#).unwrap(),
            ("", Expr::String("hello world".into()))
        );

        assert_eq!(
            parse_expr(r#""hello? world""#).unwrap(),
            ("", Expr::String("hello? world".into()))
        );

        assert_eq!(parse_expr("; hello\n\n\n1").unwrap(), ("", num_f!(1.0)));
        assert_eq!(parse_expr("1 ; hello").unwrap(), ("", num_f!(1.0)));

        use im::vector;
        assert_eq!(
            parse_expr("(+ 1 1)").unwrap(),
            (
                "",
                Expr::List(vector![Expr::Symbol("+".into()), num_f!(1.0), num_f!(1.0)])
            )
        )
    }

    #[test]
    fn parse_ignored_input() {
        assert_eq!(ignored_input("; hello\n"), Ok(("", " hello")));
        assert_eq!(ignored_input("; hello"), Ok(("", " hello")));
        assert_eq!(ignored_input(";hello"), Ok(("", "hello")));
        assert_eq!(ignored_input(" ; hello"), Ok(("", " hello")));
    }

    #[test]
    fn test_expr_iterator() {
        let mut iter = ExprIterator::new("1 ; hello");
        let next = iter.next();
        assert!(next.is_some());
        assert_eq!(next.unwrap().unwrap(), num_f!(1.0));
        assert!(iter.next().is_none());
    }
}
