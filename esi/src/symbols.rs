use std::borrow::Cow;

use fastly::device_detection;
use fastly::http::header::{ACCEPT_LANGUAGE, COOKIE, HOST, REFERER};
use fastly::http::HeaderName;
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

use crate::string_functions::{join, string_split};

#[derive(Debug, Clone)]
pub enum EValue<'v> {
    Dict(Vec<(Cow<'v, str>, Cow<'v, str>)>), // Dict with `Cow` for both keys and values
    List(Vec<Cow<'v, str>>),                 // List of strings (borrowed or owned)
    Str(Cow<'v, str>),                       // Single string (borrowed or owned)
}

impl<'v> From<String> for EValue<'v> {
    fn from(s: String) -> Self {
        EValue::Str(Cow::Owned(s)) // Convert `String` to `Cow::Owned`
    }
}

impl<'v> From<&'v str> for EValue<'v> {
    fn from(s: &'v str) -> Self {
        EValue::Str(Cow::Borrowed(s)) // Convert `&str` to `Cow::Borrowed`
    }
}

impl<'v> From<Vec<Cow<'v, str>>> for EValue<'v> {
    fn from(v: Vec<Cow<'v, str>>) -> Self {
        EValue::List(v)
    }
}

impl<'v> From<Vec<&'v str>> for EValue<'v> {
    fn from(vec: Vec<&'v str>) -> Self {
        EValue::List(vec.into_iter().map(Cow::Borrowed).collect())
    }
}

impl<'v> From<Vec<String>> for EValue<'v> {
    fn from(vec: Vec<String>) -> Self {
        EValue::List(vec.into_iter().map(Cow::Owned).collect())
    }
}

impl<'v> From<Vec<(String, String)>> for EValue<'v> {
    fn from(vec: Vec<(String, String)>) -> Self {
        EValue::Dict(
            vec.into_iter()
                .map(|(k, v)| (Cow::Owned(k), Cow::Owned(v)))
                .collect(),
        )
    }
}

impl<'v> From<Vec<(&'v str, &'v str)>> for EValue<'v> {
    fn from(vec: Vec<(&'v str, &'v str)>) -> Self {
        EValue::Dict(
            vec.into_iter()
                .map(|(k, v)| (Cow::Borrowed(k), Cow::Borrowed(v)))
                .collect(),
        )
    }
}

impl std::fmt::Display for EValue<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EValue::Str(s) => write!(f, "{s}"),
            EValue::List(list) => {
                let joined = list.join(", ");
                write!(f, "[{joined}]")
            }
            EValue::Dict(_) => {
                let formatted = self.to_formatted_string(", ");
                write!(f, "{{{formatted}}}")
            }
        }
    }
}

#[allow(dead_code)]
impl<'v> EValue<'v> {
    fn to_formatted_string(&self, separator: &str) -> String {
        match self {
            EValue::Dict(vec) => {
                let formatted = vec
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(separator);
                formatted
            }
            EValue::Str(s) => s.to_string(),
            EValue::List(_) => String::new(),
        }
    }

    pub fn to_cookie(&self) -> String {
        self.to_formatted_string("; ")
    }

    pub fn to_qs(&self) -> String {
        self.to_formatted_string("&")
    }

    // this will avoid the need to clone the string
    pub fn as_str(&self) -> &str {
        match self {
            EValue::Str(s) => s, // Dereference `Cow` to get `&str`
            // _ => {
            //     let formatted = self.to_string();
            //     Box::leak(formatted.into_boxed_str())
            // }
            _ => "",
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            EValue::Dict(vec) => vec.is_empty(),
            EValue::List(vec) => vec.is_empty(),
            EValue::Str(s) => s.is_empty(),
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
        default: Option<Box<Symbol<'e>>>,
    },
    Text(Option<&'e str>),
}

fn is_upper_alphanumeric_or_underscore(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_numeric() || c == '_'
}

fn is_lower_alphanumeric_or_underscore(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_numeric() || c == '_'
}

fn parse_fn_name(input: &str) -> IResult<&str, &str> {
    preceded(char('$'), take_while1(is_lower_alphanumeric_or_underscore))(input)
}

fn parse_var_name(input: &str) -> IResult<&str, (&str, Option<&str>, Option<Symbol>)> {
    tuple((
        take_while1(is_upper_alphanumeric_or_underscore),
        opt(delimited(char('{'), parse_var_key, char('}'))),
        opt(preceded(char('|'), parse_fn_nested_argument)),
    ))(input)
}

fn parse_not_quoted_dollar_or_brackets(input: &str) -> IResult<&str, &str> {
    take_till(|c: char| "$'(){},".contains(c))(input)
}

fn parse_not_space_quoted_dollar_or_brackets(input: &str) -> IResult<&str, &str> {
    take_till(|c: char| c.is_whitespace() || "$'(){},".contains(c))(input)
}

fn parse_not_dollar_or_curlies(input: &str) -> IResult<&str, &str> {
    take_till(|c: char| "${},\"".contains(c))(input)
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

    let (name, key, default) = parsed;
    let default = default.map(Box::new);

    Ok((input, Symbol::Variable { name, key, default }))
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

// Tokenizes the input string into a vector of symbols.
//
// This function takes an input string and tokenizes it into a vector of `Symbol` objects.
// It repeatedly parses symbols from the input string until the entire string is processed or an error occurs.
pub fn tokenize_symbols(input: &str) -> IResult<&str, Vec<Symbol>> {
    let mut tokens = Vec::new();
    let mut remaining_input = input;

    while !remaining_input.is_empty() {
        let (input, element) = parse_symbol(remaining_input)?;

        tokens.push(element);

        // This check prevents the parser from looping infinitely
        if input == remaining_input {
            break;
        }
        remaining_input = input;
    }

    Ok((remaining_input, tokens))
}

// Handles a symbol and returns the resulting string.
//
// This function processes a given symbol based on its type and returns the corresponding string result.
// It supports processing text, functions, and variables.
// For functions, it recursively processes the arguments and resolves the function name.
// For variables, it resolves the variable name and key.
pub fn handle_symbol<'a: 'b, 'b>(req: &'a Request, symbol: &'b Symbol<'b>) -> EValue<'b> {
    match symbol {
        Symbol::Function { name, args } => resolve_fn(req, name, args),
        Symbol::Text(Some(text)) => (*text).into(),
        Symbol::Text(None) => "".into(),
        Symbol::Variable { name, key, default } => resolve_var(req, name, *key, default),
    }
}

// Processes symbols in the input string and returns the resulting string.
//
// This function tokenizes the input string into symbols, processes each symbol,
// and concatenates the results into a single result string.
pub fn process_symbols(req: &Request, input: &str) -> String {
    let input = tokenize_symbols(input).unwrap().1;

    let mut result = String::new();

    for symbol in input {
        let evalue = handle_symbol(req, &symbol);
        result.push_str(evalue.as_str());
    }

    result
}

// Resolves a function name and its arguments to a resulting string.
//
// This function takes a function name and a list of arguments, and processes the function based on its name.
fn resolve_fn<'a>(req: &'a Request, name: &'a str, args: &'a [Symbol<'a>]) -> EValue<'a> {
    let mut processed_args: Vec<EValue> = Vec::new();
    // Recursively resolve the arguments
    for arg in args {
        let evalue = handle_symbol(req, arg);
        processed_args.push(evalue);
    }

    let mut result = String::new();

    match name {
        // literals
        "dollar" => result.push('$'),
        "dquote" => result.push('"'),
        "squote" => result.push('\''),
        // string functions
        "string_split" => return string_split(&processed_args),
        "join" => return join(&processed_args),
        "rand" => {
            let n = processed_args[0]
                .as_str()
                .parse::<u32>()
                .unwrap_or(99_999_999);
            result.push_str(&rand::thread_rng().gen_range(0..n).to_string());
        }
        // "func2" => {
        //     for arg in processed_args {
        //         result.push_str(&processed_args[0].as_str());
        //     }
        // }
        _ => result.push_str("unknown_function"),
    }
    result.into()
}

// Resolves a variable to its value.
//
// This function takes a variable name, an optional key, and an optional default value,
// and resolves the variable to its value based on the provided request.
fn resolve_var<'v>(
    req: &'v Request,
    name: &str,
    key: Option<&str>,
    default: &'v Option<Box<Symbol>>,
) -> EValue<'v> {
    match name {
        // ESI w3.org 1.0 spec variables
        "HTTP_ACCEPT_LANGUAGE" => header_value(req, ACCEPT_LANGUAGE, default),
        "HTTP_COOKIE" => var_http_cookie(req, key, default),
        "HTTP_HOST" => header_value(req, HOST, default),
        "HTTP_REFERER" => header_value(req, REFERER, default),
        "HTTP_USER_AGENT" => var_http_user_agent(req, key, default),
        "QUERY_STRING" => var_query_string(req, key, default),

        // Akamai EdgeSuite 5.0 ESI variables
        "REMOTE_ADDR" => client_ip_addr()
            .map(|a| a.to_string())
            .unwrap_or_default()
            .into(),
        "REQUEST_METHOD" => req.get_method_str().into(),
        "REQUEST_PATH" => req.get_path().into(),

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
            result.into()
        }
    }
}

// Resolve the value of the QUERY_STRING variable
fn var_query_string<'v>(
    req: &'v Request,
    key: Option<&str>,
    default: &'v Option<Box<Symbol>>,
) -> EValue<'v> {
    let qs = key
        .map_or_else(
            // If no key is provided, return the entire query string as a vector of key-value pairs
            || req.get_query().map(EValue::Dict).ok(),
            // If a key is provided, return the value associated with that key in the query string
            |key| req.get_query_parameter(key).map(EValue::from),
        )
        // Turn empty query strings / params to None
        .and_then(|v| if v.is_empty() { None } else { Some(v) });
    // If None return the provided `default` value
    value_or_default(qs, req, default)
}

// Returns the provided value or a default value if the provided value is `None`.
fn value_or_default<'v>(
    value: Option<EValue<'v>>,
    req: &'v Request,
    default: &'v Option<Box<Symbol>>,
) -> EValue<'v> {
    value.unwrap_or_else(|| {
        default.as_ref().map_or_else(
            // If no value and no default is provided, return an empty string
            || EValue::from(""),
            |symbol| handle_symbol(req, symbol.as_ref()),
        )
    })
}

// Resolve the value of a header
// The key parameter is used to extract specific information from the user agent string
// The default parameter is used to provide a default value if the user agent string is empty
fn header_value<'v>(
    req: &'v Request,
    header_name: HeaderName,
    default: &'v Option<Box<Symbol>>,
) -> EValue<'v> {
    let value = req.get_header_str(header_name).map(EValue::from);
    value_or_default(value, req, default)
}

// Resolve the value of the HTTP_USER_AGENT variable
// The key parameter can be one of the following:
// - browser: returns the browser name
// - os: returns the operating system name
// - version: returns the browser version
// If the key parameter is not provided, the entire user agent string is returned
fn var_http_user_agent<'v>(
    req: &'v Request,
    key: Option<&str>,
    default: &'v Option<Box<Symbol>>,
) -> EValue<'v> {
    let user_agent = header_value(req, USER_AGENT, default);

    let Some(key) = key else {
        return user_agent;
    };

    match key {
        "browser" => {
            let device = device_detection::lookup(user_agent.as_str());
            let browser = device
                .map(|d| d.device_name().map(ToString::to_string))
                .unwrap_or_default();
            browser.unwrap_or_else(|| "OTHER".to_string()).into()
        }
        // TODO: waiting for device_detection to buble this up

        // "os" => {}
        // "version" => {}
        _ => user_agent,
    }
}

fn var_http_cookie<'v>(
    req: &'v Request,
    key: Option<&str>,
    default: &'v Option<Box<Symbol>>,
) -> EValue<'v> {
    let cookies = req.get_header_str(COOKIE).unwrap_or_default();
    let cookies = cookies
        .split(';')
        .filter_map(|cookie| cookie.trim().split_once('='))
        .collect::<Vec<(&str, &str)>>();

    if key.is_none() {
        return value_or_default(Some(cookies.into()), req, default);
    }
    let found = key
        .and_then(|key| cookies.iter().find(|(k, _)| **k == *key).map(|(_, v)| *v))
        .map(EValue::from);

    if found.is_none() {
        value_or_default(None, req, default)
    } else {
        value_or_default(found, req, default)
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
            default: None,
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
            default: None,
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
            default: None,
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
            default: None,
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
            default: None,
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
                default: None,
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
        let result = resolve_var(&req, "QUERY_STRING", None, &None);
        assert_eq!(result.to_qs(), "key1=value1&key2=value2");
    }

    #[test]
    fn test_resolve_var_query_string_with_key() {
        let req = Request::new(Method::GET, "http://example.com/?key1=value1&key2=value2");
        let result = resolve_var(&req, "QUERY_STRING", Some("key1"), &None);
        assert_eq!(result.to_qs(), "value1");
    }

    #[test]
    fn test_resolve_var_query_string_with_nonexistent_key() {
        let req = Request::new(Method::GET, "http://example.com/?key1=value1&key2=value2");
        let result = resolve_var(&req, "QUERY_STRING", Some("nonexistent"), &None);
        assert_eq!(result.to_qs(), "");
    }

    #[test]
    fn test_resolve_var_remote_addr() {
        let req = Request::from_client();
        let result = resolve_var(&req, "REMOTE_ADDR", None, &None);
        assert_eq!(result.as_str(), client_ip_addr().unwrap().to_string());
    }

    #[test]
    fn test_parse_variable_with_default_literal() {
        let input = "$(QUERY_STRING|default_value)";
        let expected = Symbol::Variable {
            name: "QUERY_STRING",
            key: None,
            default: Some(Box::new(Symbol::Text(Some("default_value")))),
        };
        let (remaining, parsed) = parse_variable(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_variable_with_default_function() {
        let input = "$(QUERY_STRING|$func1(arg1))";
        let expected = Symbol::Variable {
            name: "QUERY_STRING",
            key: None,
            default: Some(Box::new(Symbol::Function {
                name: "func1",
                args: vec![Symbol::Text(Some("arg1"))],
            })),
        };
        let (remaining, parsed) = parse_variable(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_variable_with_default_variable() {
        let input = "$(QUERY_STRING|$(OTHER_VAR))";
        let expected = Symbol::Variable {
            name: "QUERY_STRING",
            key: None,
            default: Some(Box::new(Symbol::Variable {
                name: "OTHER_VAR",
                key: None,
                default: None,
            })),
        };
        let (remaining, parsed) = parse_variable(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_variable_with_key_and_default_literal() {
        let input = "$(QUERY_STRING{key}|default_value)";
        let expected = Symbol::Variable {
            name: "QUERY_STRING",
            key: Some("key"),
            default: Some(Box::new(Symbol::Text(Some("default_value")))),
        };
        let (remaining, parsed) = parse_variable(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_variable_with_key_and_default_function() {
        let input = "$(QUERY_STRING{key}|$func1(arg1))";
        let expected = Symbol::Variable {
            name: "QUERY_STRING",
            key: Some("key"),
            default: Some(Box::new(Symbol::Function {
                name: "func1",
                args: vec![Symbol::Text(Some("arg1"))],
            })),
        };
        let (remaining, parsed) = parse_variable(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_variable_with_key_and_default_variable() {
        let input = "$(QUERY_STRING{key}|$(OTHER_VAR))";
        let expected = Symbol::Variable {
            name: "QUERY_STRING",
            key: Some("key"),
            default: Some(Box::new(Symbol::Variable {
                name: "OTHER_VAR",
                key: None,
                default: None,
            })),
        };
        let (remaining, parsed) = parse_variable(input).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(remaining, "");
    }
    #[test]
    fn test_var_http_cookie_no_cookies() {
        let req = Request::new(Method::GET, "http://example.com");
        let result = var_http_cookie(&req, None, &None);
        assert_eq!(result.to_cookie(), "");
    }

    #[test]
    fn test_var_http_cookie_single_cookie() {
        let mut req = Request::new(Method::GET, "http://example.com");
        req.set_header(COOKIE, "session=abc123");
        let result = var_http_cookie(&req, None, &None);
        assert_eq!(result.to_cookie(), "session=abc123");
    }

    #[test]
    fn test_var_http_cookie_multiple_cookies() {
        let mut req = Request::new(Method::GET, "http://example.com");
        req.set_header(COOKIE, "session=abc123; user=john; theme=dark");
        let result = var_http_cookie(&req, None, &None);
        assert_eq!(result.to_cookie(), "session=abc123; user=john; theme=dark");
    }

    #[test]
    fn test_var_http_cookie_with_key() {
        let mut req = Request::new(Method::GET, "http://example.com");
        req.set_header(COOKIE, "session=abc123; user=john");
        let result = var_http_cookie(&req, Some("user"), &None);
        assert_eq!(result.to_cookie(), "john");
    }

    #[test]
    fn test_var_http_cookie_with_nonexistent_key() {
        let mut req = Request::new(Method::GET, "http://example.com");
        req.set_header(COOKIE, "session=abc123");
        let result = var_http_cookie(&req, Some("nonexistent"), &None);
        assert_eq!(result.to_cookie(), "");
    }

    #[test]
    fn test_var_http_cookie_with_default() {
        let req = Request::new(Method::GET, "http://example.com");
        let default = Some(Box::new(Symbol::Text(Some("default_cookie"))));
        let result = var_http_cookie(&req, Some("nonexistent"), &default);
        assert_eq!(result.to_cookie(), "default_cookie");
    }
}
