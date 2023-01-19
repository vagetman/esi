//! # ESI for Fastly
//!
//! This crate provides a streaming Edge Side Includes parser and executor designed for Fastly Compute@Edge.
//!
//! The implementation is currently a subset of the [ESI Language Specification 1.0](https://www.w3.org/TR/esi-lang/), so
//! only the `esi:include` tag is supported. Other tags will be ignored.
//!
//! ## Usage Example
//!
//! ```rust,no_run
//! use esi::Processor;
//! use fastly::{http::StatusCode, mime, Error, Request, Response};
//!
//! fn main() {
//!     if let Err(err) = handle_request(Request::from_client()) {
//!         println!("returning error response");
//!
//!         Response::from_status(StatusCode::INTERNAL_SERVER_ERROR)
//!             .with_body(err.to_string())
//!             .send_to_client();
//!     }
//! }
//!
//! fn handle_request(req: Request) -> Result<(), Error> {
//!     // Fetch ESI document from backend.
//!     let beresp = req.clone_without_body().send("origin_0")?;
//!
//!     // Construct an ESI processor with the default configuration.
//!     let config = esi::Configuration::default();
//!     let mut processor = Processor::new(config);
//!
//!     // Execute the ESI document using the client request as context
//!     // and sending all requests to the backend `origin_1`.
//      processor.execute_esi(
//          req,
//          beresp,
//          &|(req, idx)| {
//              println!("Sending request {}", idx);
//              Ok(req.with_ttl(120).send_async("mock-s3")?)
//          },
//          &|(resp, idx)| {
//              println!("Received response {}", idx);
//              Ok(resp)
//          },
//      )?;
//!
//!     Ok(())
//! }
//! ```

mod config;
mod error;
mod parse;

use fastly::http::body::StreamingBody;
use fastly::http::request::PendingRequest;
use fastly::http::{header, Method, StatusCode};
use fastly::{mime, Body, Request, Response};
use log::{debug, error};
use quick_xml::{Reader, Writer};
use std::collections::VecDeque;
use std::io::Write;

use crate::error::Result;
pub use crate::parse::{parse_tags, Event, Tag};

pub use crate::config::Configuration;
pub use crate::error::ExecutionError;

/// An instance of the ESI processor with a given configuration.
pub struct Processor {
    xml_reader: Reader<Body>,
    original_request_metadata: Option<Request>,
    configuration: Configuration,
}

pub struct RequestMeta {
    idx: usize,
    alt: Option<String>,
    continue_on_error: bool,
}

pub enum Element {
    Raw(Vec<u8>),
    Fragment((PendingRequest, usize)),
}

impl std::fmt::Debug for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Element::Raw(_) => write!(f, "Raw"),
            Element::Fragment(_) => write!(f, "Fragment"),
        }
    }
}

impl Processor {
    pub fn new(
        document: Body,
        original_request_metadata: Option<Request>,
        configuration: Configuration,
    ) -> Self {
        // Create a parser for the ESI document
        let xml_reader = reader_from_body(document);

        debug!("processor initialized");

        Self {
            xml_reader,
            original_request_metadata,
            configuration,
        }
    }

    pub fn execute(
        &mut self,
        dispatch_request: Option<&dyn Fn((Request, usize)) -> Result<PendingRequest>>,
        process_response: Option<&dyn Fn((Response, usize)) -> Result<Response>>,
    ) -> Result<()> {
        debug!("starting response stream");

        // Create a response to send the headers to the client
        let resp = Response::from_status(StatusCode::OK).with_content_type(mime::TEXT_HTML); // TODO: make this configurable

        // Send the response headers to the client and open an output stream
        let output_stream = resp.stream_to_client();

        // Set up an XML writer to write directly to the client output stream.
        let mut xml_writer = Writer::new(output_stream);

        debug!("parsing document");

        let mut elements: VecDeque<Element> = VecDeque::new();

        let original_request_metadata = if let Some(req) = &self.original_request_metadata {
            req.clone_without_body()
        } else {
            Request::new(Method::GET, "http://localhost")
        };

        let mut req_count = 0;

        parse_tags(
            &self.configuration.namespace.clone(),
            &mut self.xml_reader,
            &mut |event| {
                match event {
                    Event::ESI(Tag::Include {
                        src,
                        alt,
                        continue_on_error,
                    }) => {
                        debug!("got ESI");

                        req_count += 1;

                        let element = send_esi_fragment_request(
                            original_request_metadata.clone_without_body(),
                            &src,
                            dispatch_request.unwrap_or_else(|| {
                                &|_| panic!("no dispatch request function provided")
                            }),
                            req_count,
                        )?;
                        elements.push_back(element);
                    }
                    Event::XML(event) => {
                        debug!("got other content");
                        if elements.is_empty() {
                            debug!("nothing waiting so streaming directly to client");
                            xml_writer.write_event(event)?;
                            xml_writer.inner().flush().expect("failed to flush output");
                        } else {
                            debug!("pushing content to buffer, len: {}", elements.len());
                            let mut vec = Vec::new();
                            let mut writer = Writer::new(&mut vec);
                            writer.write_event(event)?;
                            elements.push_back(Element::Raw(vec));
                        }
                    }
                }

                poll_elements(&mut elements, &mut xml_writer)?;

                Ok(())
            },
        )?;

        // Wait for any pending requests to complete
        loop {
            if elements.is_empty() {
                break;
            }

            poll_elements(&mut elements, &mut xml_writer)?;
        }

        Ok(())
    }
}

fn send_esi_fragment_request(
    mut req: Request,
    url: &str,
    dispatch_request: &dyn Fn((Request, usize)) -> Result<PendingRequest>,
    idx: usize,
) -> Result<Element> {
    if url.starts_with('/') {
        req.get_url_mut().set_path(url);
    } else {
        req.set_url(url);
    }

    let hostname = req.get_url().host().expect("no host").to_string();

    req.set_header(header::HOST, &hostname);

    debug!("Requesting ESI fragment: {}", url);

    let req = match dispatch_request((req, idx)) {
        Ok(req) => req,
        Err(err) => {
            error!("Failed to dispatch request: {:?}", err);
            return Err(err);
        }
    };

    Ok(Element::Fragment((req, idx)))
}

fn poll_elements(
    elements: &mut VecDeque<Element>,
    xml_writer: &mut Writer<StreamingBody>,
) -> Result<()> {
    loop {
        let element = elements.pop_front();

        if let Some(element) = element {
            match element {
                Element::Raw(raw) => {
                    debug!("writing previously queued other content");
                    xml_writer.inner().write_all(&raw).unwrap();
                }
                Element::Fragment((pending_request, idx)) => match pending_request.poll() {
                    fastly::http::request::PollResult::Pending(pending_request) => {
                        elements.insert(0, Element::Fragment((pending_request, idx)));
                        break;
                    }
                    fastly::http::request::PollResult::Done(Ok(res)) => {
                        debug!("request poll DONE SUCCESS");
                        xml_writer
                            .inner()
                            .write_all(&res.into_body_bytes())
                            .unwrap();
                        xml_writer.inner().flush().expect("failed to flush output");
                        debug!("response {} sent to client", idx);
                    }
                    fastly::http::request::PollResult::Done(Err(err)) => todo!(),
                },
            }
        } else {
            break;
        }
    }

    Ok(())
}

fn reader_from_body(body: Body) -> Reader<Body> {
    let mut reader = Reader::from_reader(body);

    // TODO: make this configurable
    reader.check_end_names(false);

    reader
}
