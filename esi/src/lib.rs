#![doc = include_str!("../../README.md")]

mod config;
mod document;
mod error;
mod parse;

use document::{PollTaskState, Task};
use fastly::http::request::PendingRequest;
use fastly::http::{header, Method, StatusCode, Url};
use fastly::{mime, Body, Request, Response};
use log::{debug, error, trace};
use std::collections::VecDeque;
use std::io::{BufRead, Write};

pub use crate::document::{Element, Fragment};
pub use crate::error::Result;
pub use crate::parse::{parse_tags, Event, Include, Tag, Tag::Try};

pub use crate::config::Configuration;
pub use crate::error::ExecutionError;

// re-export quick_xml Reader and Writer
pub use quick_xml::{Reader, Writer};

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
    pub const fn new(
        original_request_metadata: Option<Request>,
        configuration: Configuration,
    ) -> Self {
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
        let original_request_metadata = self.original_request_metadata.as_ref().map_or_else(
            || Request::new(Method::GET, "http://localhost"),
            Request::clone_without_body,
        );

        let is_escaped = self.configuration.is_escaped;
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
                            is_escaped,
                        );
                        let alt_req = alt.map(|alt| {
                            build_fragment_request(
                                original_request_metadata.clone_without_body(),
                                &alt,
                                is_escaped,
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
                    Event::ESI(Tag::Try {
                        attempt_events,
                        except_events,
                    }) => {
                        let attempt_task = parse_task(
                            attempt_events,
                            is_escaped,
                            &original_request_metadata,
                            dispatch_fragment_request,
                        )?;
                        let except_task = parse_task(
                            except_events,
                            is_escaped,
                            &original_request_metadata,
                            dispatch_fragment_request,
                        )?;

                        // push the elements
                        elements.push_back(Element::Try {
                            attempt_task,
                            except_task,
                        });
                    }
                    Event::XML(event) => {
                        if elements.is_empty() {
                            debug!("nothing waiting so streaming directly to client");
                            output_writer.write_event(event)?;
                            output_writer
                                .get_mut()
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

fn parse_task(
    events: Vec<Event>,
    is_escaped: bool,
    original_request_metadata: &Request,
    dispatch_fragment_request: &FragmentRequestDispatcher,
) -> Result<Task> {
    let mut task = Task::new();
    for event in events {
        if let Event::ESI(Tag::Include {
            ref src,
            ref alt,
            ref continue_on_error,
        }) = event
        {
            let req = build_fragment_request(
                original_request_metadata.clone_without_body(),
                src,
                is_escaped,
            );
            let alt_req = alt.clone().map(|alt| {
                build_fragment_request(
                    original_request_metadata.clone_without_body(),
                    &alt,
                    is_escaped,
                )
            });

            if let Some(fragment) =
                send_fragment_request(req?, alt_req, *continue_on_error, dispatch_fragment_request)?
            {
                // build up task list with fragments
                task.queue.push_back(Element::Include(fragment));
            }
        }
        if let Event::XML(event) = event {
            debug!("XML event inside esi:try -- {event:?}");
            debug!(
                "pushing non-ESI content to task's buffer, len: {}",
                task.queue.len()
            );
            let mut vec = Vec::new();
            let mut writer = Writer::new(&mut vec);
            writer.write_event(event)?;
            task.queue.push_back(Element::Raw(vec));
        }
    }
    Ok(task)
}

fn build_fragment_request(mut request: Request, url: &str, is_escaped: bool) -> Result<Request> {
    let escaped_url = if is_escaped {
        match quick_xml::escape::unescape(url) {
            Ok(url) => url.to_string(),
            Err(err) => {
                return Err(ExecutionError::InvalidRequestUrl(err.to_string()));
            }
        }
    } else {
        url.to_string()
    };

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
#[allow(clippy::cognitive_complexity)]
fn poll_elements(
    elements: &mut VecDeque<Element>,
    output_writer: &mut Writer<impl Write>,
    dispatch_fragment_request: &FragmentRequestDispatcher,
    process_fragment_response: Option<&FragmentResponseProcessor>,
) -> Result<()> {
    while let Some(element) = elements.pop_front() {
        match element {
            Element::Raw(raw) => {
                debug!("writing previously queued other content");
                output_writer.get_mut().write_all(&raw).unwrap();
            }
            Element::Include(Fragment {
                mut request,
                alt,
                continue_on_error,
                pending_request,
            }) => {
                match pending_request.wait() {
                    Ok(res) => {
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
                                .get_mut()
                                .write_all(&res.into_body_bytes())
                                .unwrap();
                            output_writer
                                .get_mut()
                                .flush()
                                .expect("failed to flush output");
                        } else {
                            // Response status is NOT success, either continue, fallback to an alt, or fail.
                            if let Some(request) = alt {
                                debug!("request poll DONE ERROR, trying alt");
                                if let Some(fragment) = send_fragment_request(
                                    request?,
                                    None,
                                    continue_on_error,
                                    dispatch_fragment_request,
                                )? {
                                    // push the request back to front with ALT as the request
                                    elements.push_front(Element::Include(fragment));
                                    break;
                                }
                                debug!("guest returned None, continuing");
                                continue;
                            } else if continue_on_error {
                                debug!("request poll DONE ERROR, NO ALT, continuing");
                                continue;
                            }
                            debug!("request poll DONE ERROR, NO ALT, failing");
                            return Err(ExecutionError::UnexpectedStatus(
                                request.get_url_str().to_string(),
                                res.get_status().into(),
                            ));
                        }
                    }
                    Err(err) => return Err(ExecutionError::RequestError(err)),
                }
            }

            Element::Try {
                mut attempt_task,
                mut except_task,
            } => {
                let attempt_state = poll_tasks(
                    &mut attempt_task,
                    dispatch_fragment_request,
                    process_fragment_response,
                )?;
                let except_state = poll_tasks(
                    &mut except_task,
                    dispatch_fragment_request,
                    process_fragment_response,
                )?;

                match (attempt_state, except_state) {
                    (PollTaskState::Succeeded, _) => {
                        output_handler(output_writer, &attempt_task.output.into_inner());
                        continue;
                    }
                    (PollTaskState::Failed(_, _), PollTaskState::Succeeded) => {
                        output_handler(output_writer, &except_task.output.into_inner());
                        continue;
                    }
                    (PollTaskState::Failed(req, res), PollTaskState::Failed(_req, _res)) => {
                        // both tasks failed
                        return Err(ExecutionError::UnexpectedStatus(
                            req.get_url_str().to_string(),
                            res,
                        ));
                    }
                    (PollTaskState::Pending, _) | (_, PollTaskState::Pending) => {
                        // Request are still pending, re-add it to the front of the queue and wait for the next poll.
                        elements.push_front(Element::Try {
                            attempt_task,
                            except_task,
                        });
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

fn poll_tasks(
    task: &mut Task,
    dispatch_fragment_request: &FragmentRequestDispatcher,
    process_fragment_response: Option<&FragmentResponseProcessor>,
) -> Result<PollTaskState> {
    // return the Failed status if it's already known
    if let PollTaskState::Failed(_, _) = &task.status {
        debug!("The task has previously failed, returning failed status");
        return Ok(task.status.clone());
    }
    // loop over elements of the task
    while let Some(element) = task.queue.pop_front() {
        let (mut request, alt, continue_on_error, pending_request) = match element {
            Element::Include(Fragment {
                request,
                alt,
                continue_on_error,
                pending_request,
            }) => (request, alt, continue_on_error, pending_request),
            Element::Raw(raw) => {
                task.output.get_mut().extend_from_slice(&raw);
                continue;
            }
            Element::Try {
                attempt_task,
                except_task,
            } => {
                let mut nested_try = VecDeque::from(vec![Element::Try {
                    attempt_task,
                    except_task,
                }]);

                poll_elements(
                    &mut nested_try,
                    &mut task.output,
                    dispatch_fragment_request,
                    process_fragment_response,
                )?;

                continue;
            }
        };

        match pending_request.wait() {
            Ok(res) => {
                let res = if let Some(process_response) = process_fragment_response {
                    process_response(&mut request, res)?
                } else {
                    res
                };

                if res.get_status().is_success() {
                    trace!(
                        "Poll is success, {} - {}",
                        request.get_url_str(),
                        res.get_status()
                    );
                    task.output
                        .get_mut()
                        .extend_from_slice(&res.into_body_bytes());
                    continue;
                }
                // Response status is NOT success, either continue, fallback to an alt, or fail.
                if let Some(req) = alt {
                    debug!("request poll DONE ERROR, trying alt");
                    if let Some(fragment) = send_fragment_request(
                        req?,
                        None,
                        continue_on_error,
                        dispatch_fragment_request,
                    )? {
                        // push the request back to front with ALT as the request
                        task.queue.push_front(Element::Include(fragment));
                        return Ok(PollTaskState::Pending);
                    }
                    debug!("guest returned None, continuing");
                    continue;
                }
                if continue_on_error {
                    debug!("request poll DONE ERROR, NO ALT, continuing");
                    continue;
                }
                debug!("request poll DONE ERROR, NO ALT, failing");
                task.status = PollTaskState::Failed(request, res.get_status().into());
                return Ok(task.status.clone());
            }
            Err(err) => return Err(ExecutionError::RequestError(err)),
        }
    }
    // no more elements, return success
    Ok(PollTaskState::Succeeded)
}

// Helper function to create an XML reader from a body.
fn reader_from_body(body: Body) -> Reader<Body> {
    let mut reader = Reader::from_reader(body);

    // TODO: make this configurable
    let config = reader.config_mut();
    config.check_end_names = false;

    reader
}

// helper function to drive output to a response stream
fn output_handler(output_writer: &mut Writer<impl Write>, buffer: &[u8]) {
    output_writer.get_mut().write_all(buffer).unwrap();
    output_writer
        .get_mut()
        .flush()
        .expect("failed to flush output");
}
