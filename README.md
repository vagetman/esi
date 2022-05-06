# esi

A streaming Edge Side Includes parser and executor designed for Fastly Compute@Edge.

The implementation is currently a subset of the [ESI Language Specification 1.0](https://www.w3.org/TR/esi-lang/).

## Supported Tags

- `<esi:include>` (+ `alt`, `onerror="continue"`)
- `<esi:comment>`
- `<esi:remove>`

## Usage

See an example app in the [`esi_example_app`](./esi_example_app/src/main.rs) subdirectory.

Full usage guide coming soon.

## License

The source and documentation for this project are released under the [MIT License](LICENSE).
