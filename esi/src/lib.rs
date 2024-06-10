#![doc = include_str!("../../README.md")]

mod config;
mod document;
mod error;
mod parse;

use document::Tasks;
use fastly::http::request::{PendingRequest, PollResult};
use fastly::http::{header, Method, StatusCode, Url};
use fastly::{mime, Body, Request, Response};
use log::{debug, error};
use quick_xml::{Reader, Writer};
use std::collections::VecDeque;
use std::io::{BufRead, Write};

pub use crate::document::{Element, Fragment};
pub use crate::error::Result;
pub use crate::parse::{parse_tags, Event, Include, Tag, Tag::Try};

pub use crate::config::Configuration;
pub use crate::error::ExecutionError;

type FragmentRequestDispatcher = dyn Fn(Request) -> Result<Option<PendingRequest>>;

type FragmentResponseProcessor = dyn Fn(&mut Request, Response) -> Result<Response>;

/// An instance of the ESI processor with a given configuration.
pub struct Processor {
    // The original client request metadata, if any.
    original_request_metadata: Option<Request>,
    // The configuration for the processor.
    configuration: Configuration,
}

impl Processor {
    pub fn new(original_request_metadata: Option<Request>, configuration: Configuration) -> Self {
        Self {
            original_request_metadata,
            configuration,
        }
    }

    /// Process a response body as an ESI document. Consumes the response body.
    pub fn process_response(
        self,
        src_document: &mut Response,
        client_response_metadata: Option<Response>,
        dispatch_fragment_request: Option<&FragmentRequestDispatcher>,
        process_fragment_response: Option<&FragmentResponseProcessor>,
    ) -> Result<()> {
        // Create a response to send the headers to the client
        let resp = client_response_metadata.unwrap_or_else(|| {
            Response::from_status(StatusCode::OK).with_content_type(mime::TEXT_HTML)
        });

        // Send the response headers to the client and open an output stream
        let output_writer = resp.stream_to_client();

        // Set up an XML writer to write directly to the client output stream.
        let mut xml_writer = Writer::new(output_writer);

        match self.process_document(
            reader_from_body(src_document.take_body()),
            &mut xml_writer,
            dispatch_fragment_request,
            process_fragment_response,
        ) {
            Ok(()) => {
                xml_writer.into_inner().finish().unwrap();
                Ok(())
            }
            Err(err) => {
                error!("error processing ESI document: {}", err);
                Err(err)
            }
        }
    }

    /// Process an ESI document from a [`quick_xml::Reader`].
    pub fn process_document(
        self,
        mut src_document: Reader<impl BufRead>,
        output_writer: &mut Writer<impl Write>,
        dispatch_fragment_request: Option<&FragmentRequestDispatcher>,
        process_fragment_response: Option<&FragmentResponseProcessor>,
    ) -> Result<()> {
        // Set up fragment request dispatcher. Use what's provided or use a default
        let dispatch_fragment_request = dispatch_fragment_request.unwrap_or({
            &|req| {
                debug!("no dispatch method configured, defaulting to hostname");
                let backend = req
                    .get_url()
                    .host()
                    .unwrap_or_else(|| panic!("no host in request: {}", req.get_url()))
                    .to_string();
                let pending_req = req.send_async(backend)?;
                Ok(Some(pending_req))
            }
        });

        // Set up the queue of document elements to be sent to the client.
        let mut elements: VecDeque<Element> = VecDeque::new();

        // If there is a source request to mimic, copy its metadata, otherwise use a default request.
        let original_request_metadata = if let Some(req) = &self.original_request_metadata {
            req.clone_without_body()
        } else {
            Request::new(Method::GET, "http://localhost")
        };

        // Begin parsing the source document
        parse_tags(
            &self.configuration.namespace,
            &mut src_document,
            &mut |event| {
                debug!("got {:?}", event);
                match event {
                    Event::ESI(Tag::Include {
                        src,
                        alt,
                        continue_on_error,
                    }) => {
                        let req = build_fragment_request(
                            original_request_metadata.clone_without_body(),
                            &src,
                        );
                        let alt_req = alt.map(|alt| {
                            build_fragment_request(
                                original_request_metadata.clone_without_body(),
                                &alt,
                            )
                        });

                        if let Some(fragment) = send_fragment_request(
                            req?,
                            alt_req,
                            continue_on_error,
                            dispatch_fragment_request,
                        )? {
                            elements.push_back(Element::Include(fragment));
                        }
                    }
                    Event::ESI(Tag::Try { attempts, excepts }) => {
                        // TODO: this will only support `esi:include` and ignore the rest of
                        // raw data for now. It needs a new home, queued right way with includes
                        let mut attempt_task = Tasks::new();
                        let mut except_task = Tasks::new();
                        for attempt in attempts {
                            if let Event::ESI(Tag::Include {
                                ref src,
                                ref alt,
                                ref continue_on_error,
                            }) = attempt
                            {
                                let req = build_fragment_request(
                                    original_request_metadata.clone_without_body(),
                                    &src,
                                );
                                let alt_req = alt.clone().map(|alt| {
                                    build_fragment_request(
                                        original_request_metadata.clone_without_body(),
                                        &alt,
                                    )
                                });

                                if let Some(fragment) = send_fragment_request(
                                    req?,
                                    alt_req,
                                    *continue_on_error,
                                    dispatch_fragment_request,
                                )? {
                                    // build up task list with fragments
                                    attempt_task.include.push_back(fragment);
                                }
                            }
                            if let Event::XML(event) = attempt {
                                if elements.is_empty() {
                                    debug!("nothing waiting so streaming directly to client");
                                    output_writer.write_event(event)?;
                                    output_writer
                                        .inner()
                                        .flush()
                                        .expect("failed to flush output");
                                } else {
                                    debug!("pushing content to buffer, len: {}", elements.len());
                                    let mut vec = Vec::new();
                                    let mut writer = Writer::new(&mut vec);
                                    writer.write_event(event)?;
                                    attempt_task.raw.extend_from_slice(&vec);
                                }
                            }
                        }
                        for except in excepts {
                            if let Event::ESI(Tag::Include {
                                ref src,
                                ref alt,
                                ref continue_on_error,
                            }) = except
                            {
                                let req = build_fragment_request(
                                    original_request_metadata.clone_without_body(),
                                    &src,
                                );
                                let alt_req = alt.clone().map(|alt| {
                                    build_fragment_request(
                                        original_request_metadata.clone_without_body(),
                                        &alt,
                                    )
                                });

                                if let Some(fragment) = send_fragment_request(
                                    req?,
                                    alt_req,
                                    *continue_on_error,
                                    dispatch_fragment_request,
                                )? {
                                    // build up task list with fragments
                                    except_task.include.push_back(fragment);
                                }
                            }
                            if let Event::XML(event) = except {
                                if elements.is_empty() {
                                    debug!("nothing waiting so streaming directly to client");
                                    output_writer.write_event(event)?;
                                    output_writer
                                        .inner()
                                        .flush()
                                        .expect("failed to flush output");
                                } else {
                                    debug!("pushing content to buffer, len: {}", elements.len());
                                    let mut vec = Vec::new();
                                    let mut writer = Writer::new(&mut vec);
                                    writer.write_event(event)?;
                                    except_task.raw.extend_from_slice(&vec);
                                }
                            }
                        }

                        // push the elements
                        elements.push_back(Element::Try {
                            attempt_tasks: attempt_task,
                            except_tasks: except_task,
                            attempt_failed: false,
                        })
                    }
                    Event::XML(event) => {
                        if elements.is_empty() {
                            debug!("nothing waiting so streaming directly to client");
                            output_writer.write_event(event)?;
                            output_writer
                                .inner()
                                .flush()
                                .expect("failed to flush output");
                        } else {
                            debug!("pushing content to buffer, len: {}", elements.len());
                            let mut vec = Vec::new();
                            let mut writer = Writer::new(&mut vec);
                            writer.write_event(event)?;
                            elements.push_back(Element::Raw(vec));
                        }
                    }
                }

                poll_elements(
                    &mut elements,
                    output_writer,
                    dispatch_fragment_request,
                    process_fragment_response,
                )?;

                Ok(())
            },
        )?;

        // Wait for any pending requests to complete
        loop {
            if elements.is_empty() {
                break;
            }

            poll_elements(
                &mut elements,
                output_writer,
                dispatch_fragment_request,
                process_fragment_response,
            )?;
        }

        Ok(())
    }
}

fn build_fragment_request(mut request: Request, url: &str) -> Result<Request> {
    let escaped_url = match quick_xml::escape::unescape(url) {
        Ok(url) => url,
        Err(err) => {
            return Err(ExecutionError::InvalidRequestUrl(err.to_string()));
        }
    }
    .to_string();

    if escaped_url.starts_with('/') {
        match Url::parse(
            format!("{}://0.0.0.0{}", request.get_url().scheme(), escaped_url).as_str(),
        ) {
            Ok(u) => {
                request.get_url_mut().set_path(u.path());
                request.get_url_mut().set_query(u.query());
            }
            Err(_err) => {
                return Err(ExecutionError::InvalidRequestUrl(escaped_url));
            }
        };
    } else {
        request.set_url(match Url::parse(&escaped_url) {
            Ok(url) => url,
            Err(_err) => {
                return Err(ExecutionError::InvalidRequestUrl(escaped_url));
            }
        });
    }

    let hostname = request.get_url().host().expect("no host").to_string();

    request.set_header(header::HOST, &hostname);

    Ok(request)
}

fn send_fragment_request(
    req: Request,
    alt: Option<Result<Request>>,
    continue_on_error: bool,
    dispatch_request: &FragmentRequestDispatcher,
) -> Result<Option<Fragment>> {
    debug!("Requesting ESI fragment: {}", req.get_url());

    let request = req.clone_without_body();

    let pending_request = match dispatch_request(req) {
        Ok(Some(req)) => req,
        Ok(None) => {
            debug!("No pending request returned, skipping");
            return Ok(None);
        }
        Err(err) => {
            error!("Failed to dispatch request: {:?}", err);
            return Err(err);
        }
    };

    Ok(Some(Fragment {
        request,
        alt,
        continue_on_error,
        pending_request,
    }))
}

// This function is responsible for polling pending requests and writing their
// responses to the client output stream. It also handles any queued source
// content that needs to be written to the client output stream.
fn poll_elements(
    elements: &mut VecDeque<Element>,
    output_writer: &mut Writer<impl Write>,
    dispatch_fragment_request: &FragmentRequestDispatcher,
    process_fragment_response: Option<&FragmentResponseProcessor>,
) -> Result<()> {
    loop {
        let Some(element) = elements.pop_front() else {
            break;
        };

        match element {
            Element::Raw(raw) => {
                debug!("writing previously queued other content");
                output_writer.inner().write_all(&raw).unwrap();
            }
            Element::Include(Fragment {
                mut request,
                alt,
                continue_on_error,
                pending_request,
            }) => {
                match pending_request.poll() {
                    PollResult::Pending(pending_request) => {
                        // Request is still pending, re-add it to the front of the queue and wait for the next poll.
                        elements.push_front(Element::Include(Fragment {
                            request,
                            alt,
                            continue_on_error,
                            pending_request,
                        }));
                        break;
                    }
                    PollResult::Done(Ok(res)) => {
                        // Let the app process the response if needed.
                        let res = if let Some(process_response) = process_fragment_response {
                            process_response(&mut request, res)?
                        } else {
                            res
                        };

                        // Request has completed, check the status code.
                        if res.get_status().is_success() {
                            // Response status is success, write the response body to the output stream.
                            output_writer
                                .inner()
                                .write_all(&res.into_body_bytes())
                                .unwrap();
                            output_writer
                                .inner()
                                .flush()
                                .expect("failed to flush output");
                        } else {
                            // Response status is NOT success, either continue, fallback to an alt, or fail.
                            if let Some(alt) = alt {
                                debug!("request poll DONE ERROR, trying alt");
                                if let Some(pending_request) = dispatch_fragment_request(alt?)? {
                                    elements.push_front(Element::Include(Fragment {
                                        request,
                                        alt: None,
                                        continue_on_error,
                                        pending_request,
                                    }));
                                    break;
                                } else {
                                    debug!("guest returned None, continuing");
                                    continue;
                                }
                            } else if continue_on_error {
                                debug!("request poll DONE ERROR, NO ALT, continuing");
                                continue;
                            } else {
                                debug!("request poll DONE ERROR, NO ALT, failing");
                                return Err(ExecutionError::UnexpectedStatus(
                                    request.get_url_str().to_string(),
                                    res.get_status().into(),
                                ));
                            }
                        }
                    }
                    PollResult::Done(Err(err)) => return Err(ExecutionError::RequestError(err)),
                }
            }

            Element::Try {
                mut attempt_failed,
                mut attempt_tasks,
                mut except_tasks,
            } => {
                let mut attempts_completed = false;
                let mut excepts_completed = false;

                // check on the rest of `attempt_tasks`, unless
                // if one of `esi:include` has failed,
                if !attempt_failed {
                    // make sure all the attempt tasks are completed
                    // before moving the result to `Element::Raw`
                    loop {
                        let Some(Fragment {
                            mut request,
                            alt,
                            continue_on_error,
                            pending_request,
                        }) = attempt_tasks.include.pop_front()
                        else {
                            break;
                        };

                        match pending_request.poll() {
                            PollResult::Pending(pending_request) => {
                                // Request is still pending, re-add it to the front of the queue and wait for the next poll.
                                attempt_tasks.include.push_front(Fragment {
                                    request,
                                    alt,
                                    continue_on_error,
                                    pending_request,
                                });
                                break;
                            }
                            PollResult::Done(Ok(res)) => {
                                // Let the app process the response if needed.
                                let res = if let Some(process_response) = process_fragment_response
                                {
                                    process_response(&mut request, res)?
                                } else {
                                    res
                                };

                                // Request has completed, check the status code.
                                if res.get_status().is_success() {
                                    // Response status is success, write the response body to the attempt buffer.
                                    attempt_tasks.raw.extend_from_slice(&res.into_body_bytes());
                                } else {
                                    // Response status is NOT success, either continue, fallback to an alt, or fail.
                                    if let Some(alt) = alt {
                                        debug!("request poll DONE ERROR, trying alt");
                                        if let Some(pending_request) =
                                            dispatch_fragment_request(alt?)?
                                        {
                                            // Remove `alt` and put it back to front
                                            attempt_tasks.include.push_front(Fragment {
                                                request,
                                                alt: None,
                                                continue_on_error,
                                                pending_request,
                                            });

                                            break;
                                        } else {
                                            debug!("guest returned None, continuing");
                                            continue;
                                        }
                                    } else if continue_on_error {
                                        debug!("request poll DONE ERROR, NO ALT, continuing");
                                        continue;
                                    } else {
                                        debug!("request poll DONE ERROR, NO ALT, failing attempt");
                                        attempt_failed = true;
                                    }
                                }
                            }
                            PollResult::Done(Err(err)) => {
                                return Err(ExecutionError::RequestError(err))
                            }
                        }
                        // }
                    }
                    attempts_completed = true;
                }

                // check on the rest of `except_tasks` only when
                // attempt tasks have not completed
                if !attempts_completed {
                    loop {
                        let Some(Fragment {
                            mut request,
                            alt,
                            continue_on_error,
                            pending_request,
                        }) = except_tasks.include.pop_front()
                        else {
                            break;
                        };
                        match pending_request.poll() {
                            PollResult::Pending(pending_request) => {
                                // Request is still pending, re-add it to the front of the queue and wait for the next poll.
                                except_tasks.include.push_front(Fragment {
                                    request,
                                    alt,
                                    continue_on_error,
                                    pending_request,
                                });
                                break;
                            }
                            PollResult::Done(Ok(res)) => {
                                // Let the app process the response if needed.
                                let res = if let Some(process_response) = process_fragment_response
                                {
                                    process_response(&mut request, res)?
                                } else {
                                    res
                                };

                                // Request has completed, check the status code.
                                if res.get_status().is_success() {
                                    // Response status is success, write the response body to the attempt buffer.
                                    except_tasks.raw.extend_from_slice(&res.into_body_bytes());
                                    excepts_completed = true;
                                } else {
                                    // Response status is NOT success, either continue, fallback to an alt, or fail.
                                    if let Some(alt) = alt {
                                        debug!("request poll DONE ERROR, trying alt");
                                        if let Some(pending_request) =
                                            dispatch_fragment_request(alt?)?
                                        {
                                            // Re-build the `Task` with updated Fragment, without `alt`
                                            except_tasks.include.push_front(Fragment {
                                                request,
                                                alt: None,
                                                continue_on_error: continue_on_error,
                                                pending_request,
                                            });

                                            break;
                                        } else {
                                            debug!("guest returned None, continuing");
                                            continue;
                                        }
                                    } else if continue_on_error {
                                        debug!("request poll DONE ERROR, NO ALT, continuing");
                                        continue;
                                    } else {
                                        debug!("request poll DONE ERROR, NO ALT, failing");
                                        return Err(ExecutionError::UnexpectedStatus(
                                            request.get_url_str().to_string(),
                                            res.get_status().into(),
                                        ));
                                    }
                                }
                            }
                            PollResult::Done(Err(err)) => {
                                return Err(ExecutionError::RequestError(err))
                            }
                        }
                    }
                }

                // if attempt tasks have completed
                // write the responses to the output stream.
                if attempts_completed {
                    output_handler(output_writer, &attempt_tasks.raw);
                } else if excepts_completed && attempt_failed {
                    output_handler(output_writer, &except_tasks.raw);
                } else {
                    // Request are still pending, re-add it to the front of the queue and wait for the next poll.
                    elements.push_front(Element::Try {
                        attempt_failed,
                        attempt_tasks,
                        except_tasks,
                    });
                }
                break;
            }
        }
    }

    Ok(())
}

// Helper function to create an XML reader from a body.
fn reader_from_body(body: Body) -> Reader<Body> {
    let mut reader = Reader::from_reader(body);

    // TODO: make this configurable
    reader.check_end_names(false);

    reader
}
// helper function to drive output to a response stream
fn output_handler(output_writer: &mut Writer<impl Write>, buffer: &[u8]) {
    output_writer.inner().write_all(&buffer).unwrap();
    output_writer
        .inner()
        .flush()
        .expect("failed to flush output");
}
