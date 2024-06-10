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

#[test]
fn parse_try_accept_only_include() -> Result<(), ExecutionError> {
    setup();

    let input = "<esi:try><esi:attempt><esi:include src=\"abc\" alt=\"def\" onerror=\"continue\"/></esi:attempt></esi:try>";
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

    assert!(!parsed);

    Ok(())
}

#[test]
fn parse_try_accept_except_include() -> Result<(), ExecutionError> {
    setup();

    let input = r#"
<esi:try>
    <esi:attempt>
        <esi:include src="/abc"/>
    </esi:attempt>
    <esi:except>
        <esi:include src="/xyz"/>
        <a href="/efg"/>
        just text
    </esi:except>
</esi:try>"#;
    let mut plain_include_parsed = false;
    let mut accept_include_parsed = false;
    let mut except_include_parsed = false;

    parse_tags("esi", &mut Reader::from_str(input), &mut |event| {
        println!("Event - {event:?}");
        if let Event::ESI(Tag::Include {
            ref src,
            ref alt,
            ref continue_on_error,
        }) = event
        {
            assert_eq!(src, &"/foo");
            assert_eq!(alt, &None);
            assert!(!continue_on_error);
            plain_include_parsed = true;
        }
        if let Event::ESI(Tag::Try {
            attempts: attempt,
            excepts: except,
        }) = event
        {
            // process accept tasks
            for attempt_event in attempt {
                if let Event::ESI(Tag::Include {
                    src,
                    alt,
                    continue_on_error,
                }) = attempt_event
                {
                    assert_eq!(src, "/abc");
                    assert_eq!(alt, None);
                    assert!(!continue_on_error);
                    accept_include_parsed = true;
                }
            }
            // process except tasks
            for except_event in except {
                if let Event::ESI(Tag::Include {
                    src,
                    alt,
                    continue_on_error,
                }) = except_event
                {
                    assert_eq!(src, "/xyz");
                    assert_eq!(alt, None);
                    assert!(!continue_on_error);
                    except_include_parsed = true;
                }
            }
        }

        Ok(())
    })?;

    assert!(!plain_include_parsed);
    assert!(accept_include_parsed);

    Ok(())
}
