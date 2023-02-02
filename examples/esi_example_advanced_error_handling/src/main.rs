use fastly::{http::StatusCode, mime, Request, Response};
use log::{error, info};
use quick_xml::{Reader, Writer};

fn main() {
    env_logger::builder()
        .filter(None, log::LevelFilter::Trace)
        .init();

    let req = Request::from_client();

    if req.get_path() != "/" {
        Response::from_status(StatusCode::NOT_FOUND).send_to_client();
        return;
    }

    // Generate synthetic test response from "index.html" file.
    // You probably want replace this with a backend call, e.g. `req.clone_without_body().send("origin_0")`
    let mut beresp =
        Response::from_body(include_str!("index.html")).with_content_type(mime::TEXT_HTML);

    // If the response is HTML, we can parse it for ESI tags.
    if beresp
        .get_content_type()
        .map(|c| c.subtype() == mime::HTML)
        .unwrap_or(false)
    {
        let processor = esi::Processor::new(Some(req), esi::Configuration::default());

        // Create a response to send the headers to the client
        let resp = Response::from_status(StatusCode::OK).with_content_type(mime::TEXT_HTML);

        // Send the response headers to the client and open an output stream
        let output_writer = resp.stream_to_client();

        // Set up an XML writer to write directly to the client output stream.
        let mut xml_writer = Writer::new(output_writer);

        match processor.process_document(
            Reader::from_reader(beresp.take_body()),
            &mut xml_writer,
            Some(&|req| {
                info!("Sending request {} {}", req.get_method(), req.get_path());
                Ok(Some(req.with_ttl(120).send_async("mock-s3")?))
            }),
            Some(&|req, resp| {
                info!(
                    "Received response for {} {}",
                    req.get_method(),
                    req.get_path()
                );
                Ok(resp)
            }),
        ) {
            Ok(()) => {
                xml_writer.into_inner().finish().unwrap();
            }
            Err(err) => {
                error!("error processing ESI document: {}", err);
                xml_writer
                    .inner()
                    .write_str(include_str!("error.html.fragment"));
                xml_writer.into_inner().finish().unwrap_or_else(|_| {
                    error!("error flushing error response to client");
                });
            }
        }
    } else {
        // Otherwise, we can just return the response.
        beresp.send_to_client();
    }
}
