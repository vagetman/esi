use esi::Processor;
use fastly::{http::StatusCode, mime, Error, Request, Response};

fn main() {
    env_logger::builder()
        .filter(None, log::LevelFilter::Trace)
        .init();

    if let Err(err) = handle_request(Request::from_client()) {
        println!("returning error response");

        Response::from_status(StatusCode::INTERNAL_SERVER_ERROR)
            .with_body(err.to_string())
            .send_to_client();
    }
}

fn handle_request(req: Request) -> Result<(), Error> {
    if req.get_path() != "/" {
        Response::from_status(StatusCode::NOT_FOUND).send_to_client();
        return Ok(());
    }

    // Generate synthetic test response from "index.html" file.
    let beresp = Response::from_body(include_str!("index.html")).with_content_type(mime::TEXT_HTML);

    let config = esi::Configuration::default().with_recursion();

    let processor = Processor::new(config);

    processor.execute_esi(req, beresp, &|req| {
        Ok(req.with_ttl(120).send("esi-test.edgecompute.app")?)
    })?;

    Ok(())
}
