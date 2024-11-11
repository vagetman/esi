use std::borrow::Cow;

use fastly::device_detection;
use fastly::http::header::COOKIE;
use fastly::{handle::client_ip_addr, http::header::USER_AGENT, Request};
use nom::{
    branch::alt,
    bytes::complete::{tag, take_till, take_while1},
    character::complete::{char, multispace0},
    combinator::{map, opt},
    multi::separated_list0,
    sequence::{delimited, preceded, terminated, tuple},
    IResult,
};
use rand::Rng;

pub(crate) enum EValue<'v> {
    AmpersandSeparatedKv(Vec<(String, String)>),
    CommaSeparatedKv(Vec<(String, String)>),
    CommaSeparatedValues(Vec<String>),
    Str(&'v str),
    String(String),
    CookieList(Vec<(&'v str, &'v str)>),
}

impl<'v> EValue<'v> {
    // this will avoid the need to clone the string
    fn as_str(&self) -> Cow<str> {
        match self {
            EValue::Str(s) => Cow::Borrowed(s),
            EValue::String(s) => Cow::Borrowed(s.as_str()),
            EValue::AmpersandSeparatedKv(vec) => {
                let kv_strings = vec.iter().map(|(k, v)| format!("{k}={v}"));
                kv_strings.collect::<Vec<String>>().join("&").into()
            }
            EValue::CommaSeparatedKv(vec) => {
                let kv_strings = vec.iter().map(|(k, v)| format!("{k}={v}"));
                kv_strings.collect::<Vec<String>>().join(", ").into()
            }
            EValue::CookieList(vec) => {
                let kv_strings = vec.iter().map(|(k, v)| format!("{k}={v}"));
                kv_strings.collect::<Vec<String>>().join("; ").into()
            }
            _ => Cow::Borrowed(""),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Symbol<'e> {
    Function {
        name: &'e str,
        args: Vec<Symbol<'e>>,
    },
    Variable {
        name: &'e str,
        key: Option<&'e str>,
        // default: Option<Box<Symbol<'e>>>,
    },
    Text(Option<&'e str>),
}

// fn is_alphanumeric_or_underscore(c: char) -> bool {
//     c.is_alphanumeric() || c.is_numeric() || c == '_'
// }

fn is_upper_alphanumeric_or_underscore(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_numeric() || c == '_'
}

fn is_lower_alphanumeric_or_underscore(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_numeric() || c == '_'
}

fn parse_fn_name(input: &str) -> IResult<&str, &str> {
    preceded(char('$'), take_while1(is_lower_alphanumeric_or_underscore))(input)
}

fn parse_var_name(input: &str) -> IResult<&str, (&str, Option<&str>)> {
    tuple((
        take_while1(is_upper_alphanumeric_or_underscore),
        opt(delimited(char('{'), parse_var_key, char('}'))),
    ))(input)
}

fn parse_not_quoted_dollar_or_brackets(input: &str) -> IResult<&str, &str> {
    take_till(|c: char| {
        c == '$' || c == '\'' || c == '(' || c == ')' || c == '{' || c == '}' || c == ','
    })(input)
}

fn parse_not_space_quoted_dollar_or_brackets(input: &str) -> IResult<&str, &str> {
    take_till(|c: char| {
        c.is_whitespace()
            || c == '$'
            || c == '\''
            || c == '('
            || c == ')'
            || c == '{'
            || c == '}'
            || c == ','
    })(input)
}

fn parse_not_dollar_or_curlies(input: &str) -> IResult<&str, &str> {
    take_till(|c: char| c == '$' || c == '{' || c == '}' || c == ',' || c == '"')(input)
}

fn parse_single_quoted_ascii(input: &str) -> IResult<&str, &str> {
    take_till(|c: char| c == '\'' || !c.is_ascii())(input)
}

fn parse_text(input: &str) -> IResult<&str, &str> {
    alt((
        delimited(char('\''), parse_single_quoted_ascii, char('\'')),
        parse_not_quoted_dollar_or_brackets,
    ))(input)
}

fn parse_fn_text(input: &str) -> IResult<&str, &str> {
    alt((
        delimited(char('\''), parse_single_quoted_ascii, char('\'')),
        parse_not_space_quoted_dollar_or_brackets,
    ))(input)
}

fn parse_var_key(input: &str) -> IResult<&str, &str> {
    alt((
        delimited(char('\''), parse_single_quoted_ascii, char('\'')),
        parse_not_dollar_or_curlies,
    ))(input)
}

fn parse_fn_argument(input: &str) -> IResult<&str, Vec<Symbol>> {
    let (input, mut parsed) = separated_list0(
        tuple((multispace0, char(','), multispace0)),
        parse_fn_nested_argument,
    )(input)?;

    // If the parsed list contains a single empty text element return an empty vec
    if parsed.len() == 1 && parsed[0] == Symbol::Text(None) {
        parsed = vec![];
    }
    Ok((input, parsed))
}

fn parse_fn_nested_argument(input: &str) -> IResult<&str, Symbol> {
    alt((
        parse_function,
        parse_variable,
        map(parse_fn_text, |text| {
            if text.is_empty() {
                Symbol::Text(None)
            } else {
                Symbol::Text(Some(text))
            }
        }),
    ))(input)
}

fn parse_function(input: &str) -> IResult<&str, Symbol> {
    let (input, parsed) = tuple((
        parse_fn_name,
        delimited(
            terminated(char('('), multispace0),
            parse_fn_argument,
            preceded(multispace0, char(')')),
        ),
    ))(input)?;

    let (name, args) = parsed;

    Ok((input, Symbol::Function { name, args }))
}

fn parse_variable(input: &str) -> IResult<&str, Symbol> {
    let (input, parsed) = delimited(tag("$("), parse_var_name, char(')'))(input)?;

    let (name, key) = parsed;

    Ok((input, Symbol::Variable { name, key }))
}

fn parse_symbol(input: &str) -> IResult<&str, Symbol> {
    alt((
        parse_function,
        parse_variable,
        map(parse_text, |text| {
            if text.is_empty() {
                Symbol::Text(None)
            } else {
                Symbol::Text(Some(text))
            }
        }),
    ))(input)
}

pub fn tokenize_symbols(input: &str) -> IResult<&str, Vec<Symbol>> {
    let mut tokens = Vec::new();
    let mut remaining_input = input;

    while !remaining_input.is_empty() {
        let (input, element) = parse_symbol(remaining_input)?;

        println!("Parsed element: {:?}", element);
        tokens.push(element);

        // This check prevents the parser from looping infinitely
        if input == remaining_input {
            break;
        }
        remaining_input = input;
    }

    Ok((remaining_input, tokens))
}

pub fn handle_symbol(req: &Request, symbol: Symbol) -> String {
    let mut output = String::new();
    match symbol {
        Symbol::Text(Some(text)) => output.push_str(text),
        Symbol::Text(None) => {}
        Symbol::Function { name, args } => {
            let mut processed_args = Vec::new();
            // Recursively process the arguments
            for arg in args {
                processed_args.push(handle_symbol(req, arg));
            }
            let result = resolve_fn(name, processed_args);
            output.push_str(&result);
        }
        Symbol::Variable { name, key } => {
            let result = resolve_var(req, name, key);
            output.push_str(&result.as_str());
        }
    }
    output
}

pub fn process_symbols(req: &Request, input: &str) -> String {
    let input = tokenize_symbols(input).unwrap().1;

    let mut output = String::new();

    for symbol in input {
        output.push_str(&handle_symbol(req, symbol));
    }

    output
}

fn resolve_fn(name: &str, args: Vec<String>) -> String {
    let mut result = String::new();

    match name {
        "rand" => {
            let n = args[0].parse::<u32>().unwrap_or(99999999);
            result.push_str(&rand::thread_rng().gen_range(0..n).to_string());
        }
        "func2" => {
            for arg in args {
                result.push_str(&arg);
            }
        }
        _ => result.push_str("unknown_function"),
    }
    result
}

fn resolve_var<'v>(req: &'v Request, name: &str, key: Option<&str>) -> EValue<'v> {
    match name {
        // ESI w3.org 1.0 spec variables
        "HTTP_ACCEPT_LANGUAGE" => {
            EValue::Str(req.get_header_str("Accept-Language").unwrap_or_default())
        }
        "HTTP_COOKIE" => var_http_cookie(req, key),
        "HTTP_HOST" => EValue::Str(req.get_header_str("Host").unwrap_or_default()),
        "HTTP_REFERER" => EValue::Str(req.get_header_str("Referer").unwrap_or_default()),
        "HTTP_USER_AGENT" => var_http_user_agent(req, key),
        "QUERY_STRING" => key.map_or_else(
            || EValue::AmpersandSeparatedKv(req.get_query().unwrap_or_default()),
            |key| EValue::Str(req.get_query_parameter(key).unwrap_or_default()),
        ),
        // Akamai 5.0 ESI variables
        "REMOTE_ADDR" => EValue::String(client_ip_addr().unwrap().to_string()),
        "REQUEST_METHOD" => EValue::Str(req.get_method_str()),
        "REQUEST_PATH" => EValue::Str(req.get_path()),

        // "TRAFFIC_INFO" => {}
        // "GEO" => {}
        // "HTTP_ACCEPT" => {}
        // "HTTP_ACCEPT_CHARSET" => {}
        // "HTTP_ACCEPT_ENCODING" => {}
        // "HTTP_ACCEPT_LANGUAGE" => {}
        // "HTTP_AUTHORIZATION" => {}
        // "HTTP_CACHE_CONTROL" => {}
        // "HTTP_CONNECTION" => {}
        _ => {
            let result =
                key.map_or_else(|| format!("$({name})"), |key| format!("$({name}{{{key}}})"));
            EValue::String(result)
        }
    }
}

fn var_http_user_agent<'v>(req: &'v Request, key: Option<&str>) -> EValue<'v> {
    let user_agent = req.get_header_str(USER_AGENT).unwrap_or_default();
    key.map_or(EValue::Str(user_agent), |key| match key {
        "browser" => {
            let device = device_detection::lookup(user_agent);
            let browser = device
                .map(|d| d.device_name().map(|browser| browser.to_string()))
                .unwrap_or_default();
            EValue::String(browser.unwrap_or("OTHER".to_string()))
        }

        // TODO: waiting for device_detection to buble this up

        // "os" => {}
        // "version" => {}
        _ => EValue::Str(user_agent),
    })
}

fn var_http_cookie<'v>(req: &'v Request, key: Option<&str>) -> EValue<'v> {
    let cookies = req.get_header_str(COOKIE).unwrap_or_default();
    let cookies = cookies
        .split(';')
        .map(|cookie| {
            let mut parts = cookie.split('=');
            let key = parts.next().unwrap_or_default().trim();
            let value = parts.next().unwrap_or_default().trim();
            (key, value)
        })
        .collect::<Vec<(&str, &str)>>();

    if let Some(key) = key {
        let value = cookies
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
            .unwrap_or_default();
        EValue::Str(value)
    } else {
        EValue::CookieList(cookies)
    }
}

#[cfg(test)]
mod tests {
    use fastly::http::Method;

    use super::*;

    #[test]
    fn test_parse_text() {
        let input = "some text without functions";
        let expected = "some text without functions";
        let (remaining, parsed) = parse_text(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_fn_name() {
        let input = "$func_name";
        let expected = "func_name";
        let (remaining, parsed) = parse_fn_name(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_function() {
        let input = "$func1(arg1, $func2(arg2a, arg2b), arg3)";
        let expected = Symbol::Function {
            name: "func1",
            args: vec![
                Symbol::Text(Some("arg1")),
                Symbol::Function {
                    name: "func2",
                    args: vec![Symbol::Text(Some("arg2a")), Symbol::Text(Some("arg2b"))],
                },
                Symbol::Text(Some("arg3")),
            ],
        };
        let (remaining, parsed) = parse_function(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_esi_object_simple_text() {
        let input = "simple_text";
        let expected = Symbol::Text(Some("simple_text"));
        let result = parse_symbol(input);
        assert_eq!(result, Ok(("", expected)));
    }

    #[test]
    fn test_parse_esi_object_with_function() {
        let input = "$func(inner_arg)";
        let expected = Symbol::Function {
            name: "func",
            args: vec![Symbol::Text(Some("inner_arg"))],
        };
        let result = parse_symbol(input);
        assert_eq!(result, Ok(("", expected)));
    }

    #[test]
    fn test_parse_fn_argument_with_nested_function() {
        let input = "$func(arg1, $func2(inner_arg2))";
        let expected = Symbol::Function {
            name: "func",
            args: vec![
                Symbol::Text(Some("arg1")),
                Symbol::Function {
                    name: "func2",
                    args: vec![Symbol::Text(Some("inner_arg2"))],
                },
            ],
        };
        let result = parse_symbol(input);
        assert_eq!(result, Ok(("", expected)));
    }

    #[test]
    fn test_parse_function_with_empty_arguments() {
        let input = "$func()";
        let expected = Symbol::Function {
            name: "func",
            args: vec![],
        };
        let (remaining, parsed) = parse_function(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_variable_valid() {
        let input = "$(QUERY_STRING)";
        let expected = Symbol::Variable {
            name: "QUERY_STRING",
            key: None,
        };
        let (remaining, parsed) = parse_variable(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_variable_with_key() {
        let input = "$(QUERY_STRING{first})";
        let expected = Symbol::Variable {
            name: "QUERY_STRING",
            key: Some("first"),
        };
        let (remaining, parsed) = parse_variable(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_variable_with_quoted_key() {
        let input = "$(QUERY_STRING{'first'})";
        let expected = Symbol::Variable {
            name: "QUERY_STRING",
            key: Some("first"),
        };
        let (remaining, parsed) = parse_variable(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_variable_with_double_quoted_key() {
        let input = "$(QUERY_STRING{\"first\"})";
        let result = parse_variable(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_variable_invalid_no_parentheses() {
        let input = "$QUERY_STRING";
        let result = parse_variable(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_variable_invalid_no_dollar() {
        let input = "(QUERY_STRING)";
        let result = parse_variable(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_variable_invalid_lowercase() {
        let input = "$(query_string)";
        let result = parse_variable(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_variable_with_underscore() {
        let input = "$(QUERY_STRING_WITH_UNDERSCORE)";
        let expected = Symbol::Variable {
            name: "QUERY_STRING_WITH_UNDERSCORE",
            key: None,
        };
        let (remaining, parsed) = parse_variable(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_variable_with_underscore_in_key() {
        let input = "$(QUERY_STRING{first_name})";
        let expected = Symbol::Variable {
            name: "QUERY_STRING",
            key: Some("first_name"),
        };
        let (remaining, parsed) = parse_variable(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_mixed_content() {
        let input = "Text before $func1(arg1) and $(QUERY_STRING{key}) after.";
        let expected = vec![
            Symbol::Text(Some("Text before ")),
            Symbol::Function {
                name: "func1",
                args: vec![Symbol::Text(Some("arg1"))],
            },
            Symbol::Text(Some(" and ")),
            Symbol::Variable {
                name: "QUERY_STRING",
                key: Some("key"),
            },
            Symbol::Text(Some(" after.")),
        ];
        let (remaining, parsed) = tokenize_symbols(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }
    #[test]
    fn test_tokenize_symbols_nested_functions() {
        let input = "Here is some text $func1( arg1, $func2(arg2a, arg2b ), arg3) and more text.";
        let expected = vec![
            Symbol::Text(Some("Here is some text ")),
            Symbol::Function {
                name: "func1",
                args: vec![
                    Symbol::Text(Some("arg1")),
                    Symbol::Function {
                        name: "func2",
                        args: vec![Symbol::Text(Some("arg2a")), Symbol::Text(Some("arg2b"))],
                    },
                    Symbol::Text(Some("arg3")),
                ],
            },
            Symbol::Text(Some(" and more text.")),
        ];
        let (remaining, parsed) = tokenize_symbols(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_empty_input() {
        let input = "";
        let expected: Vec<Symbol> = vec![];
        let (remaining, parsed) = tokenize_symbols(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_only_text() {
        let input = "Just some plain text.";
        let expected = vec![Symbol::Text(Some("Just some plain text."))];
        let (remaining, parsed) = tokenize_symbols(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_only_function() {
        let input = "$func1( arg1, arg2 )";
        let expected = vec![Symbol::Function {
            name: "func1",
            args: vec![Symbol::Text(Some("arg1")), Symbol::Text(Some("arg2"))],
        }];
        let (remaining, parsed) = tokenize_symbols(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_tokenize_symbols_single_nested_functions() {
        let input = "$outer($inner1(arg1), $inner2(arg2))";
        let expected = vec![Symbol::Function {
            name: "outer",
            args: vec![
                Symbol::Function {
                    name: "inner1",
                    args: vec![Symbol::Text(Some("arg1"))],
                },
                Symbol::Function {
                    name: "inner2",
                    args: vec![Symbol::Text(Some("arg2"))],
                },
            ],
        }];
        let (remaining, parsed) = tokenize_symbols(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_text_with_function_in_middle() {
        let input = "Start $func1(arg1) end";
        let expected = vec![
            Symbol::Text(Some("Start ")),
            Symbol::Function {
                name: "func1",
                args: vec![Symbol::Text(Some("arg1"))],
            },
            Symbol::Text(Some(" end")),
        ];
        let (remaining, parsed) = tokenize_symbols(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_multiple_functions() {
        let input = "$func1(arg1) $func2(arg2)";
        let expected = vec![
            Symbol::Function {
                name: "func1",
                args: vec![Symbol::Text(Some("arg1"))],
            },
            Symbol::Text(Some(" ")),
            Symbol::Function {
                name: "func2",
                args: vec![Symbol::Text(Some("arg2"))],
            },
        ];
        let (remaining, parsed) = tokenize_symbols(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_function_with_no_args() {
        let input = "$func1()";
        let expected = vec![Symbol::Function {
            name: "func1",
            args: vec![],
        }];
        let (remaining, parsed) = tokenize_symbols(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_text_with_special_characters() {
        let input = "Text with special characters '!@#$%^&*()'";
        let expected = vec![
            Symbol::Text(Some("Text with special characters ")),
            Symbol::Text(Some("!@#$%^&*()")),
        ];
        let (remaining, parsed) = tokenize_symbols(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_resolve_var_query_string() {
        let req = Request::new(Method::GET, "http://example.com/?key1=value1&key2=value2");
        let result = resolve_var(&req, "QUERY_STRING", None);
        assert_eq!(result.as_str(), "key1=value1&key2=value2");
    }

    #[test]
    fn test_resolve_var_query_string_with_key() {
        let req = Request::new(Method::GET, "http://example.com/?key1=value1&key2=value2");
        let result = resolve_var(&req, "QUERY_STRING", Some("key1"));
        assert_eq!(result.as_str(), "value1");
    }

    #[test]
    fn test_resolve_var_query_string_with_nonexistent_key() {
        let req = Request::new(Method::GET, "http://example.com/?key1=value1&key2=value2");
        let result = resolve_var(&req, "QUERY_STRING", Some("nonexistent"));
        assert_eq!(result.as_str(), "");
    }

    #[test]
    fn test_resolve_var_remote_addr() {
        let req = Request::from_client();
        let result = resolve_var(&req, "REMOTE_ADDR", None);
        assert_eq!(result.as_str(), client_ip_addr().unwrap().to_string());
    }
}
