use crate::{ExecutionError, Result};
use log::debug;
use quick_xml::events::{BytesStart, Event as XmlEvent};
use quick_xml::name::QName;
use quick_xml::Reader;
use std::io::BufRead;

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

    let mut remove = false;
    let mut open_include = false;

    let esi_include = format!("{namespace}:include",).into_bytes();
    let esi_comment = format!("{namespace}:comment",).into_bytes();
    let esi_remove = format!("{namespace}:remove",).into_bytes();
    let esi_try = format!("{namespace}:try",).into_bytes();
    let esi_attempt = format!("{namespace}:attempt",).into_bytes();
    let esi_except = format!("{namespace}:except",).into_bytes();

    let mut buffer = Vec::new();
    // Parse tags and build events vec
    loop {
        match reader.read_event_into(&mut buffer) {
            // Handle <esi:remove> tags
            Ok(XmlEvent::Start(elem)) if elem.starts_with(&esi_remove) => {
                remove = true;
            }
            Ok(XmlEvent::End(elem)) if elem.starts_with(&esi_remove) => {
                if !remove {
                    return Err(ExecutionError::UnexpectedClosingTag(
                        String::from_utf8(elem.to_vec()).unwrap(),
                    ));
                }

                remove = false;
            }
            _ if remove => continue,

            // Handle <esi:include> tags, and ignore the contents if they are not self-closing
            Ok(XmlEvent::Empty(elem)) if elem.name().into_inner().starts_with(&esi_include) => {
                callback(Event::ESI(parse_include(&elem)?))?;
            }

            Ok(XmlEvent::Start(elem)) if elem.name().into_inner().starts_with(&esi_include) => {
                open_include = true;
                callback(Event::ESI(parse_include(&elem)?))?;
            }

            Ok(XmlEvent::End(elem)) if elem.name().into_inner().starts_with(&esi_include) => {
                if !open_include {
                    return Err(ExecutionError::UnexpectedClosingTag(
                        String::from_utf8(elem.to_vec()).unwrap(),
                    ));
                }

                open_include = false;
            }

            _ if open_include => continue,

            // Ignore <esi:comment> tags
            Ok(XmlEvent::Empty(elem)) if elem.name().into_inner().starts_with(&esi_comment) => {
                continue
            }

            // Handle <esi:try> tags
            Ok(XmlEvent::Start(ref elem)) if elem.name() == QName(&esi_try) => {
                parse_try(
                    reader,
                    callback,
                    &esi_include,
                    &esi_attempt,
                    &esi_except,
                    &esi_try,
                )?;
            }

            Ok(XmlEvent::Eof) => {
                debug!("End of document");
                break;
            }
            Ok(e) => callback(Event::XML(e.into_owned()))?,
            _ => {}
        }
    }

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
        .map(|attr| &attr.value.to_vec() == b"continue")
        == Some(true);

    Ok(Tag::Include {
        src,
        alt,
        continue_on_error,
    })
}

fn parse_try<'a, R>(
    reader: &mut Reader<R>,
    callback: &mut dyn FnMut(Event<'a>) -> Result<()>,
    esi_include: &[u8],
    esi_attempt: &[u8],
    esi_except: &[u8],
    esi_try: &[u8],
) -> Result<()>
where
    R: BufRead,
{
    #[derive(Debug, PartialEq)]
    enum TryNestedTag {
        Attempt,
        Except,
    }
    let mut inside_tag: Option<TryNestedTag> = None;

    let mut buf = Vec::new();
    let mut attempt_events = Vec::new();
    let mut except_events = Vec::new();
    let mut attempt_found = false;
    let mut except_found = false;
    let mut open_include = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(ref e)) if e.name() == QName(esi_attempt) => {
                if inside_tag.is_some() || attempt_found {
                    return Err(ExecutionError::UnexpectedOpeningTag(
                        String::from_utf8(e.to_vec()).unwrap(),
                    ));
                }
                inside_tag = Some(TryNestedTag::Attempt);
                attempt_found = true;
            }
            Ok(XmlEvent::Start(ref e)) if e.name() == QName(esi_except) => {
                if !attempt_found || inside_tag.is_some() || except_found {
                    return Err(ExecutionError::UnexpectedOpeningTag(
                        String::from_utf8(e.to_vec()).unwrap(),
                    ));
                }
                inside_tag = Some(TryNestedTag::Except);
                except_found = true;
            }
            // Handle <esi:include> tags, and ignore the contents if they are not self-closing
            Ok(XmlEvent::Start(elem)) if elem.name().into_inner().starts_with(esi_include) => {
                let tag = parse_include(&elem)?;
                match inside_tag {
                    Some(TryNestedTag::Attempt) => attempt_events.push(Event::ESI(tag)),
                    Some(TryNestedTag::Except) => except_events.push(Event::ESI(tag)),
                    _ => (),
                }
                open_include = true;
            }

            Ok(XmlEvent::Empty(elem)) if elem.name().into_inner().starts_with(esi_include) => {
                let tag = parse_include(&elem)?;
                match inside_tag {
                    Some(TryNestedTag::Attempt) => attempt_events.push(Event::ESI(tag)),
                    Some(TryNestedTag::Except) => except_events.push(Event::ESI(tag)),
                    _ => (),
                }
            }

            Ok(XmlEvent::End(elem)) if elem.name().into_inner().starts_with(esi_include) => {
                if !open_include {
                    return Err(ExecutionError::UnexpectedClosingTag(
                        String::from_utf8(elem.to_vec()).unwrap(),
                    ));
                }

                open_include = false;
            }
            Ok(XmlEvent::End(ref e)) if e.name() == QName(esi_attempt) => {
                inside_tag = None;
            }
            Ok(XmlEvent::End(ref e)) if e.name() == QName(esi_except) => {
                inside_tag = None;
            }
            Ok(XmlEvent::End(ref e)) if e.name() == QName(esi_try) => {
                if !attempt_found {
                    return Err(ExecutionError::UnexpectedClosingTag(
                        String::from_utf8(e.to_vec()).unwrap(),
                    ));
                }
                callback(Event::ESI(Tag::Try {
                    attempt_events,
                    except_events,
                }))?;
                break;
            }
            Ok(XmlEvent::Text(txt)) => {
                println!("Inside tag -- {inside_tag:?}");
                if inside_tag.is_none() {
                    // no inner content allowed outside `esi:attempt` or `esi:exempt` tags
                    continue;
                }
                match inside_tag {
                    Some(TryNestedTag::Attempt) => {
                        attempt_events.push(Event::XML(XmlEvent::Text(txt.into_owned())));
                    }
                    Some(TryNestedTag::Except) => {
                        except_events.push(Event::XML(XmlEvent::Text(txt.into_owned())));
                    }
                    _ => (),
                }
            }
            Ok(event) if inside_tag == Some(TryNestedTag::Attempt) => {
                attempt_events.push(Event::XML(event.into_owned()));
            }
            Ok(event) if inside_tag == Some(TryNestedTag::Except) => {
                except_events.push(Event::XML(event.into_owned()));
            }
            Ok(event) => {
                callback(Event::XML(event.into_owned()))?;
            }
            Err(err) => return Err(err.into()),
        }

        buf.clear();
    }

    Ok(())
}
