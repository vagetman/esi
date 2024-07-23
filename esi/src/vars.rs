#![allow(unused)]
use fastly::Request;
use log::{debug, error, trace};

use nom::branch::alt;
use nom::bytes::complete::{is_not, tag, take_till, take_until, take_while, take_while1};
use nom::character::complete::{char, none_of, space0};
use nom::combinator::{complete, eof, map, opt, peek, value};
use nom::multi::{many0, many1, many_till};
use nom::sequence::{delimited, pair, preceded, separated_pair, tuple};
use nom::IResult;

use crate::error::Result;
use crate::esi_dollar_structs::EsiData;
use crate::parse;

pub fn resolve_vars<'a>(input: &'a str, request: &'a Request) -> Result<String> {
    let parse_result = parse_vars(input, request);

    let vars = match parse_result {
        IResult::Ok((_, vars)) => vars,
        IResult::Err(_) => {
            debug!("Failed to parse vars");
            return Ok(input.to_string());
        }
    };

    Ok(vars)
}

// eg
// \$(HTTP_HOST),
// the pcice is \$100.00
//
fn parse_text(input: &str) -> IResult<&str, &str> {
    fn parse_literal(input: &str) -> IResult<&str, &str> {
        take_while1(|c: char| c != '\\' && c != '$')(input)
    }

    fn escaped_dollar(input: &str) -> IResult<&str, &str> {
        preceded(tag("\\$"), parse_literal)(input)
    }

    alt((parse_literal, escaped_dollar))(input)
}

fn parse_dollar_tag(input: &str) -> IResult<&str, &str> {
    alt((parse_var, parse_function))(input)
}

fn parse_vars<'a>(input: &'a str, req: &'a Request) -> IResult<&'a str, String> {
    map(
        many_till(
            alt((
                map(parse_text, |text| text.to_string()),
                map(parse_dollar_tag, |token| lookup_variable(token, req)),
            )),
            eof,
        ),
        |(parsed, _)| parsed.join(""),
    )(input)
}

fn parse_params(input: &str) -> IResult<&str, &str> {
    map(
        tuple((
            space0,
            tag("{"),
            delimited(tag("{"), take_until("}"), tag("}")),
        )),
        |(_, _, params)| params,
    )(input)
}

fn parse_var_with_params(input: &str) -> IResult<&str, &str> {
    map(
        delimited(tag("$("), pair(esi_variable_name, parse_params), tag(")")),
        |(_, param)| param,
    )(input)
}

fn parse_var(input: &str) -> IResult<&str, &str> {
    delimited(tag("$("), esi_variable_name, tag(")"))(input)
}

fn parse_function(input: &str) -> IResult<&str, &str> {
    delimited(tag("$("), esi_function_name, tag(")"))(input)
}

fn lookup_variable<'a>(var_name: &'a str, req: &'a Request) -> String {
    match var_name {
        "HTTP_HOST" => req.get_url().host_str().unwrap_or_default().to_string(),
        "QUERY_STRING" => req.get_url().query().unwrap_or_default().to_string(),
        // variable not found, return the dollar string with the variable name
        _ => format!("$({})", var_name),
    }
}

fn esi_variable_name(i: &str) -> IResult<&str, &str> {
    take_while1(move |c: char| c.is_ascii_uppercase() || c == '_')(i)
}

fn esi_function_name(i: &str) -> IResult<&str, &str> {
    take_while1(move |c: char| c.is_ascii_lowercase() || c == '_')(i)
}

#[cfg(test)]
#[test]
fn test_esi_variable_name() {
    let input = "HTTP_HOST";
    let result = esi_variable_name(input);
    assert_eq!(result, Ok(("", "HTTP_HOST")));
}

#[test]
fn test_esi_function_name() {
    let input = "random_function";
    let result = esi_function_name(input);
    assert_eq!(result, Ok(("", "random_function")));
}

#[cfg(test)]
#[test]
fn test_parse_vars() {
    use fastly::http::Method;

    let req = Request::new(Method::GET, "http://localhost");

    let input = "http://$(HTTP_HOST)/$(path)/file";
    let result = parse_vars(input, &req);
    // println!("{:?}", result);
    assert_eq!(
        result,
        Ok(("", "http://localhost/$(path)/file".to_string()))
    );

    let input = "http://$(nothing)/$(path)/file";
    let result = parse_vars(input, &req);
    // println!("{:?}", result);

    assert_eq!(
        result,
        Ok(("", "http://$(nothing)/$(path)/file".to_string()))
    );

    let input = "Price of this item is \\$item(q) 100.00";
    let result = parse_vars(input, &req);
    println!("{:?}", result);
}

#[test]
fn test_lookup_variable() {
    use fastly::http::Method;

    let req = Request::new(Method::GET, "http://localhost?foo=bar");

    let var_name = "QUERY_STRING";
    let result = lookup_variable(var_name, &req);
    println!("{:?}", result);
    assert_eq!(result, "foo=bar".to_string());

    let var_name = "unknown_function";
    let result = lookup_variable(var_name, &req);
    println!("{:?}", result);
    assert_eq!(result, "$(unknown_function)".to_string())
}
#[test]
fn test_resolve_vars() {
    use fastly::http::Method;

    let req = Request::new(Method::GET, "http://localhost");
    let input = "http://$(HTTP_HOST)/$(path)/file";
    let result = resolve_vars(input, &req);
    println!("result 1 = {:?}", result);
    // assert_eq!(result.unwrap(), "http://localhost/$(path)".to_string());

    let input = r#"http://localhost\/path/file"#;
    let result = resolve_vars(input, &req);
    println!("result 2 = {:?}", result);
    // assert_eq!(result.unwrap(), "http://localhost/path/file".to_string());

    let input = "the price is \\$100.00";
    let result = resolve_vars(input, &req);
    println!("result 3 = {:?}", result);
    // assert_eq!(result.unwrap(), "http://localhost/path/file".to_string());

    let input = "http://localhost/$path/file";
    let result = resolve_vars(input, &req);
    println!("result 4 = {:?}", result);
    // assert_eq!(result.unwrap(), "http://localhost/path/file".to_string());
}

#[test]
fn eof_parser() {
    use nom::error::ErrorKind;

    fn eof_or_dollar(input: &str) -> IResult<&str, &str> {
        alt((take_while(|c: char| c != '$'), eof))(input)
    }

    let parser = eof;

    assert_eq!(parser("abc"), Err(nom::Err::Error(("abc", ErrorKind::Eof))));
    assert_eq!(parser(""), Ok(("", "")));
    println!("{:?}", eof_or_dollar("abc$"));
    println!("{:?}", eof_or_dollar("abc$func()"));
    println!("{:?}", eof_or_dollar("1abc"));
}
