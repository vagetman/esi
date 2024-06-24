use crate::{ExecutionError, Result};
use log::debug;
use quick_xml::events::{BytesStart, Event as XmlEvent};
use quick_xml::name::QName;
use quick_xml::Reader;
use std::io::BufRead;

// State carrier of Try branch
#[derive(Debug, PartialEq)]
enum TryTagArms {
    Try,
    Attempt,
    Except,
}

/// Representation of an ESI tag from a source response.
#[derive(Debug)]
pub struct Include {
    pub src: String,
    pub alt: Option<String>,
    pub continue_on_error: bool,
}

#[derive(Debug)]
pub enum Tag<'a> {
    Include {
        src: String,
        alt: Option<String>,
        continue_on_error: bool,
    },
    Try {
        attempt_events: Vec<Event<'a>>,
        except_events: Vec<Event<'a>>,
    },
}

/// Representation of either XML data or a parsed ESI tag.
#[derive(Debug)]
#[allow(clippy::upper_case_acronyms)]
pub enum Event<'e> {
    XML(XmlEvent<'e>),
    ESI(Tag<'e>),
}

// #[derive(Debug)]
struct EsiTags {
    include: Vec<u8>,
    comment: Vec<u8>,
    remove: Vec<u8>,
    tryy: Vec<u8>,
    attempt: Vec<u8>,
    esi_except: Vec<u8>,
}
impl EsiTags {
    fn init(namespace: &str) -> Self {
        Self {
            include: format!("{namespace}:include",).into_bytes(),
            comment: format!("{namespace}:comment",).into_bytes(),
            remove: format!("{namespace}:remove",).into_bytes(),
            tryy: format!("{namespace}:try",).into_bytes(),
            attempt: format!("{namespace}:attempt",).into_bytes(),
            esi_except: format!("{namespace}:except",).into_bytes(),
        }
    }
}

fn do_parse<'a, R>(
    reader: &mut Reader<R>,
    callback: &mut dyn FnMut(Event<'a>) -> Result<()>,
    task: &mut Vec<Event<'a>>,
    depth: &mut usize,
    current_arm: &mut Option<TryTagArms>,
    tags: &EsiTags,
) -> Result<()>
where
    R: BufRead,
{
    let mut is_remove_tag = false;
    let mut open_include = false;

    let attempt_events = &mut Vec::new();
    let except_events = &mut Vec::new();

    let mut buffer = Vec::new();
    // Parse tags and build events vec
    loop {
        match reader.read_event_into(&mut buffer) {
            // Handle <esi:remove> tags
            Ok(XmlEvent::Start(elem)) if elem.name() == QName(&tags.remove) => {
                is_remove_tag = true;
            }

            Ok(XmlEvent::End(elem)) if elem.name() == QName(&tags.remove) => {
                if !is_remove_tag {
                    return Err(ExecutionError::UnexpectedClosingTag(
                        String::from_utf8(elem.to_vec()).unwrap(),
                    ));
                }

                is_remove_tag = false;
            }
            _ if is_remove_tag => continue,

            // Handle <esi:include> tags, and ignore the contents if they are not self-closing
            Ok(XmlEvent::Empty(elem)) if elem.name().into_inner().starts_with(&tags.include) => {
                if *depth == 0 {
                    callback(Event::ESI(parse_include(&elem)?))?;
                } else {
                    task.push(Event::ESI(parse_include(&elem)?));
                }
            }

            Ok(XmlEvent::Start(elem)) if elem.name().into_inner().starts_with(&tags.include) => {
                open_include = true;
                if *depth == 0 {
                    callback(Event::ESI(parse_include(&elem)?))?;
                } else {
                    task.push(Event::ESI(parse_include(&elem)?));
                }
            }

            Ok(XmlEvent::End(elem)) if elem.name().into_inner().starts_with(&tags.include) => {
                if !open_include {
                    return Err(ExecutionError::UnexpectedClosingTag(
                        String::from_utf8(elem.to_vec()).unwrap(),
                    ));
                }

                open_include = false;
            }

            _ if open_include => continue,

            // Ignore <esi:comment> tags
            Ok(XmlEvent::Empty(elem)) if elem.name().into_inner().starts_with(&tags.comment) => {
                continue
            }

            // Handle <esi:try> tags
            Ok(XmlEvent::Start(ref elem)) if elem.name() == QName(&tags.tryy) => {
                *current_arm = Some(TryTagArms::Try);
                *depth += 1;
                continue;
            }

            // Handle <esi:attempt> tags in recursion
            Ok(XmlEvent::Start(ref e)) if e.name() == QName(&tags.attempt) => {
                if *current_arm == Some(TryTagArms::Attempt)
                    || *current_arm == Some(TryTagArms::Except)
                {
                    return Err(ExecutionError::UnexpectedOpeningTag(
                        String::from_utf8(e.to_vec()).unwrap(),
                    ));
                }
                *current_arm = Some(TryTagArms::Attempt);
                do_parse(reader, callback, attempt_events, depth, current_arm, tags)?;
            }

            // Handle <esi:except> tags in recursion
            Ok(XmlEvent::Start(ref e)) if e.name() == QName(&tags.esi_except) => {
                if *current_arm == Some(TryTagArms::Attempt)
                    || *current_arm == Some(TryTagArms::Except)
                {
                    return Err(ExecutionError::UnexpectedOpeningTag(
                        String::from_utf8(e.to_vec()).unwrap(),
                    ));
                }
                *current_arm = Some(TryTagArms::Except);
                do_parse(reader, callback, except_events, depth, current_arm, tags)?;
            }

            Ok(XmlEvent::End(ref e)) if e.name() == QName(&tags.tryy) => {
                *current_arm = None;
                if *depth == 0 {
                    return Err(ExecutionError::UnexpectedClosingTag(
                        String::from_utf8(e.to_vec()).unwrap(),
                    ));
                }
                if *depth == 1 {
                    callback(Event::ESI(Tag::Try {
                        attempt_events: std::mem::take(attempt_events),
                        except_events: std::mem::take(except_events),
                    }))?;
                    *depth = 0;
                    continue;
                }
                *depth -= 1;
                task.push(Event::ESI(Tag::Try {
                    attempt_events: std::mem::take(attempt_events),
                    except_events: std::mem::take(except_events),
                }));
                continue;
            }

            Ok(XmlEvent::End(ref e)) if e.name() == QName(&tags.attempt) => {
                *current_arm = None;
                if *depth == 0 {
                    return Err(ExecutionError::UnexpectedClosingTag(
                        String::from_utf8(e.to_vec()).unwrap(),
                    ));
                }
                return Ok(());
            }

            Ok(XmlEvent::End(ref e)) if e.name() == QName(&tags.esi_except) => {
                *current_arm = None;
                if *depth == 0 {
                    return Err(ExecutionError::UnexpectedClosingTag(
                        String::from_utf8(e.to_vec()).unwrap(),
                    ));
                }
                return Ok(());
            }

            Ok(XmlEvent::Eof) => {
                debug!("End of document");
                break;
            }
            Ok(e) => {
                if *depth == 0 {
                    callback(Event::XML(e.into_owned()))?;
                } else {
                    task.push(Event::XML(e.into_owned()));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Parses the ESI document from the given `reader` and calls the `callback` closure upon each successfully parsed ESI tag.
pub fn parse_tags<'a, R>(
    namespace: &str,
    reader: &mut Reader<R>,
    callback: &mut dyn FnMut(Event<'a>) -> Result<()>,
) -> Result<()>
where
    R: BufRead,
{
    debug!("Parsing document...");

    // Initialize the ESI tags
    let tags = EsiTags::init(namespace);
    // set the initial depth of nested tags
    let mut depth = 0;
    let mut root = Vec::new();

    let mut current_arm: Option<TryTagArms> = None;

    do_parse(
        reader,
        callback,
        &mut root,
        &mut depth,
        &mut current_arm,
        &tags,
    )?;
    debug!("Root: {:?}", root);

    Ok(())
}

fn parse_include<'a>(elem: &BytesStart) -> Result<Tag<'a>> {
    let src = match elem
        .attributes()
        .flatten()
        .find(|attr| attr.key.into_inner() == b"src")
    {
        Some(attr) => String::from_utf8(attr.value.to_vec()).unwrap(),
        None => {
            return Err(ExecutionError::MissingRequiredParameter(
                String::from_utf8(elem.name().into_inner().to_vec()).unwrap(),
                "src".to_string(),
            ));
        }
    };

    let alt = elem
        .attributes()
        .flatten()
        .find(|attr| attr.key.into_inner() == b"alt")
        .map(|attr| String::from_utf8(attr.value.to_vec()).unwrap());

    let continue_on_error = elem
        .attributes()
        .flatten()
        .find(|attr| attr.key.into_inner() == b"onerror")
        .is_some_and(|attr| &attr.value.to_vec() == b"continue");

    Ok(Tag::Include {
        src,
        alt,
        continue_on_error,
    })
}
