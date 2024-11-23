use crate::symbols::EValue;
use std::borrow::Cow;

pub fn string_split<'a>(args: &[EValue]) -> EValue<'a> {
    // $string_split(string [,sep] [,max_sep])
    let string = args.first().map(|s| s.as_str()).unwrap_or_default();
    let sep = args.get(1).map_or(Cow::Borrowed(" "), |s| s.as_str());
    let max_sep = args
        .get(2)
        .and_then(|s| s.as_str().parse::<usize>().ok())
        .unwrap_or(usize::MAX);

    let val = string.splitn(max_sep, &*sep).map(String::from).collect();
    EValue::ListString(val)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_split_basic() {
        let input = vec![EValue::String("hello world".to_string())];
        if let EValue::ListString(result) = string_split(&input) {
            assert_eq!(result, vec!["hello", "world"]);
        } else {
            panic!("Expected ListString variant");
        }
    }

    #[test]
    fn test_string_split_custom_separator() {
        let input = vec![
            EValue::String("a,b,c".to_string()),
            EValue::String(",".to_string()),
        ];
        if let EValue::ListString(result) = string_split(&input) {
            assert_eq!(result, vec!["a", "b", "c"]);
        } else {
            panic!("Expected ListString variant");
        }
    }

    #[test]
    fn test_string_split_with_limit() {
        let input = vec![
            EValue::String("a:b:c:d".to_string()),
            EValue::String(":".to_string()),
            EValue::String("2".to_string()),
        ];
        if let EValue::ListString(result) = string_split(&input) {
            assert_eq!(result, vec!["a", "b:c:d"]);
        } else {
            panic!("Expected ListString variant");
        }
    }

    #[test]
    fn test_string_split_empty_input() {
        let input = vec![EValue::String("".to_string())];
        if let EValue::ListString(result) = string_split(&input) {
            assert_eq!(result, vec![""]);
        } else {
            panic!("Expected ListString variant");
        }
    }

    #[test]
    fn test_string_split_no_separator_found() {
        let input = vec![
            EValue::String("hello".to_string()),
            EValue::String(",".to_string()),
        ];
        if let EValue::ListString(result) = string_split(&input) {
            assert_eq!(result, vec!["hello"]);
        } else {
            panic!("Expected ListString variant");
        }
    }
}
