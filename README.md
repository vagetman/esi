# esi

A streaming Edge Side Includes parser and executor designed for Fastly Compute@Edge.

The implementation is currently a subset of the [ESI Language Specification 1.0](https://www.w3.org/TR/esi-lang/), supporting the following tags:

- `<esi:include>` (+ `alt`, `onerror="continue"`)
- `<esi:comment>`
- `<esi:remove>`

Other tags will be ignored.

## Example Usage

```rust,no_run
use esi::Processor;
use fastly::{http::StatusCode, mime, Error, Request, Response};

fn main() {
    if let Err(err) = handle_request(Request::from_client()) {
        println!("returning error response");

        Response::from_status(StatusCode::INTERNAL_SERVER_ERROR)
            .with_body(err.to_string())
            .send_to_client();
    }
}

fn handle_request(req: Request) -> Result<(), Error> {
    // Fetch ESI document from backend.
    let beresp = req.clone_without_body().send("origin_0")?;

    // If the response is HTML, we can parse it for ESI tags.
    if beresp
        .get_content_type()
        .map(|c| c.subtype() == mime::HTML)
        .unwrap_or(false)
    {
        let config = esi::Configuration::default();

        let processor = Processor::new(beresp.into_body(), Some(req), None, config);

        processor.execute(
            Some(&|req| {
                println!("Sending request {} {}", req.get_method(), req.get_path());
                Ok(Some(req.with_ttl(120).send_async("mock-s3")?))
            }),
            Some(&|req, resp| {
                println!(
                    "Received response for {} {}",
                    req.get_method(),
                    req.get_path()
                );
                Ok(resp)
            }),
        )?;

        Ok(())
    } else {
        // Otherwise, we can just return the response.
        beresp.send_to_client();
        Ok(())
    }
}
```

See a full example app in the [`esi_example_app`](./esi_example_app/src/main.rs) subdirectory, or read the hosted documentation at [docs.rs/esi](https://docs.rs/esi).

## License

The source and documentation for this project are released under the [MIT License](LICENSE).
