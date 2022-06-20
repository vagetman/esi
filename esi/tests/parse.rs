use esi::{parse_tags, Event, ExecutionError, Tag};
use quick_xml::Reader;

use std::sync::Once;

static INIT: Once = Once::new();

/// Setup function that is only run once, even if called multiple times.
fn setup() {
    INIT.call_once(env_logger::init);
}

#[test]
fn parse_basic_include() -> Result<(), ExecutionError> {
    setup();

    let input = "<html><body><esi:include src=\"https://example.com/hello\"/></body></html>";
    let mut parsed = false;

    parse_tags("esi", &mut Reader::from_str(input), &mut |event| {
        if let Event::ESI(Tag::Include {
            src,
            alt,
            continue_on_error,
        }) = event
        {
            assert_eq!(src, "https://example.com/hello");
            assert_eq!(alt, None);
            assert!(!continue_on_error);
            parsed = true;
        }
        Ok(())
    })?;

    assert!(parsed);

    Ok(())
}

#[test]
fn parse_advanced_include_with_namespace() -> Result<(), ExecutionError> {
    setup();

    let input = "<app:include src=\"abc\" alt=\"def\" onerror=\"continue\"/>";
    let mut parsed = false;

    parse_tags("app", &mut Reader::from_str(input), &mut |event| {
        if let Event::ESI(Tag::Include {
            src,
            alt,
            continue_on_error,
        }) = event
        {
            assert_eq!(src, "abc");
            assert_eq!(alt, Some("def".to_string()));
            assert!(continue_on_error);
            parsed = true;
        }
        Ok(())
    })?;

    assert!(parsed);

    Ok(())
}

#[test]
fn parse_open_include() -> Result<(), ExecutionError> {
    setup();

    let input = "<esi:include src=\"abc\" alt=\"def\" onerror=\"continue\"></esi:include>";
    let mut parsed = false;

    parse_tags("esi", &mut Reader::from_str(input), &mut |event| {
        if let Event::ESI(Tag::Include {
            src,
            alt,
            continue_on_error,
        }) = event
        {
            assert_eq!(src, "abc");
            assert_eq!(alt, Some("def".to_string()));
            assert!(continue_on_error);
            parsed = true;
        }
        Ok(())
    })?;

    assert!(parsed);

    Ok(())
}

#[test]
fn parse_invalid_include() -> Result<(), ExecutionError> {
    setup();

    let input = "<esi:include/>";

    let res = parse_tags("esi", &mut Reader::from_str(input), &mut |_| Ok(()));

    assert!(matches!(
        res,
        Err(ExecutionError::MissingRequiredParameter(_, _))
    ));

    Ok(())
}

#[test]
fn parse_basic_include_with_onerror() -> Result<(), ExecutionError> {
    setup();

    let input = "<esi:include src=\"/_fragments/content.html\" onerror=\"continue\"/>";
    let mut parsed = false;

    parse_tags("esi", &mut Reader::from_str(input), &mut |event| {
        if let Event::ESI(Tag::Include {
            src,
            alt,
            continue_on_error,
        }) = event
        {
            assert_eq!(src, "/_fragments/content.html");
            assert_eq!(alt, None);
            assert!(continue_on_error);
            parsed = true;
        }

        Ok(())
    })?;

    assert!(parsed);

    Ok(())
}
