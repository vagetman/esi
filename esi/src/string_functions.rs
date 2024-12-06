use crate::symbols::EValue;
use std::borrow::Cow;

pub fn string_split<'a>(args: &[EValue<'a>]) -> EValue<'a> {
    // $string_split(string [,sep] [,max_sep])
    let string = args.first().map(EValue::as_str).unwrap_or_default();
    let sep = args
        .get(1)
        .map_or(Cow::Borrowed(" "), |s| s.as_str().into());
    let max_sep = args
        .get(2)
        .and_then(|s| s.as_str().parse::<usize>().ok())
        .unwrap_or(usize::MAX);

    let val = string
        .splitn(max_sep, &*sep)
        .map(|s| Cow::Owned(s.to_string()))
        .collect::<Vec<_>>();
    val.into()
}

pub fn join<'a>(args: &[EValue<'a>]) -> EValue<'a> {
    // $join(list [,sep])
    if args.is_empty() {
        return EValue::from("");
    }
    let Some(arg0) = args.first() else {
        return EValue::from("");
    };

    let EValue::List(list) = arg0 else {
        return arg0.as_str().to_string().into();
    };
    let sep = args
        .get(1)
        .map_or(Cow::Borrowed(" "), |s| s.as_str().into());

    let val = list.join(&*sep);
    val.into()
}

pub fn index<'a>(args: &[EValue<'a>]) -> EValue<'a> {
    // $index(hay: string, needle: char)
    println!("args: {:?}", args);
    if args.is_empty() || args.len() != 2 {
        return EValue::Number(-1);
    }
    let Some(arg0) = args.first() else {
        return EValue::Number(-1);
    };
    let Some(arg1) = args.get(1) else {
        return EValue::Number(-1);
    };

    let EValue::Str(hay) = arg0 else {
        return EValue::Number(-1);
    };

    let EValue::Char(needle) = arg1 else {
        return EValue::Number(-1);
    };

    println!("hay: {}, needle: {}", hay, needle);

    let index = hay
        .as_ref()
        .chars()
        .position(|c| c == *needle)
        .map(|i| i as i32);
    let index = index.unwrap_or(-1);
    index.into()
}

fn rindex<'a>(args: &[EValue<'a>]) -> EValue<'a> {
    // $rindex(hay: string, needle: char)
    if args.is_empty() || args.len() != 2 {
        return EValue::Number(-1);
    }
    let Some(arg0) = args.first() else {
        return EValue::Number(-1);
    };
    let Some(arg1) = args.get(1) else {
        return EValue::Number(-1);
    };

    let EValue::Str(hay) = arg0 else {
        return EValue::Number(-1);
    };

    let EValue::Char(needle) = arg1 else {
        return EValue::Number(-1);
    };

    println!("hay: {}, needle: {}", hay, needle);

    let index = hay
        .as_ref()
        .chars()
        .rposition(|c| c == *needle)
        .map(|i| i as i32);
    let index = index.unwrap_or(-1);
    index.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_split_basic() {
        let input = vec![EValue::from("hello world")];
        if let EValue::List(result) = string_split(&input) {
            assert_eq!(result, vec!["hello", "world"]);
        } else {
            panic!("Expected ListString variant");
        }
    }

    #[test]
    fn test_string_split_custom_separator() {
        let input = vec![EValue::from("a,b,c"), EValue::from(",")];
        if let EValue::List(result) = string_split(&input) {
            assert_eq!(result, vec!["a", "b", "c"]);
        } else {
            panic!("Expected ListString variant");
        }
    }

    #[test]
    fn test_string_split_with_limit() {
        let input = vec![
            EValue::from("a:b:c:d"),
            EValue::from(":"),
            EValue::from("2"),
        ];
        if let EValue::List(result) = string_split(&input) {
            assert_eq!(result, vec!["a", "b:c:d"]);
        } else {
            panic!("Expected ListString variant");
        }
    }

    #[test]
    fn test_string_split_empty_input() {
        let input = vec![EValue::from("")];
        if let EValue::List(result) = string_split(&input) {
            assert_eq!(result, vec![""]);
        } else {
            panic!("Expected ListString variant");
        }
    }

    #[test]
    fn test_string_split_no_separator_found() {
        let input = vec![EValue::from("hello"), EValue::from(",")];
        if let EValue::List(result) = string_split(&input) {
            assert_eq!(result, vec!["hello"]);
        } else {
            panic!("Expected ListString variant");
        }
    }
    #[test]
    fn test_join_empty_list() {
        let args = vec![];
        let result = join(&args);
        assert_eq!(result.as_str(), "");
    }

    #[test]
    fn test_join_single_element() {
        let args = vec![EValue::from("hello")];
        let result = join(&args);
        assert_eq!(result.as_str(), "hello");
    }

    #[test]
    fn test_join_with_custom_separator() {
        let args = vec![EValue::from(vec!["a", "b", "c"]), EValue::from("|")];
        let result = join(&args);
        assert_eq!(result.as_str(), "a|b|c");
    }

    #[test]
    fn test_join_with_empty_separator() {
        let args = vec![EValue::from(vec!["hello", "world"]), EValue::from("")];
        let result = join(&args);
        assert_eq!(result.as_str(), "helloworld");
    }

    #[test]
    fn test_index_basic() {
        let args = vec![EValue::from("hello"), EValue::from('e')];
        let result = index(&args);
        assert_eq!(result.as_number(), 1);
    }

    #[test]
    fn test_index_not_found() {
        let args = vec![EValue::from("hello"), EValue::from('x')];
        let result = index(&args);
        assert_eq!(result.as_number(), -1);
    }

    #[test]
    fn test_index_empty_haystack() {
        let args = vec![EValue::from(""), EValue::from('a')];
        let result = index(&args);
        assert_eq!(result.as_number(), -1);
    }

    #[test]
    fn test_index_multiple_occurrences() {
        let args = vec![EValue::from("banana"), EValue::from('a')];
        let result = index(&args);
        assert_eq!(result.as_number(), 1);
    }

    #[test]
    fn test_index_empty_args() {
        let args: Vec<EValue> = vec![];
        let result = index(&args);
        assert_eq!(result.as_number(), -1);
    }

    #[test]
    fn test_index_invalid_args() {
        let args = vec![EValue::from("hello")];
        let result = index(&args);
        assert_eq!(result.as_number(), -1);
    }
}
