use crate::Result;
use fastly::{http::request::PendingRequest, Request};

/// A section of the pending response, either raw XML data or a pending fragment request.
pub enum Element {
    Raw(Vec<u8>),
    Fragment(
        // Metadata of the request
        Request,
        // An optional alternate request to send if the original request fails
        Option<Result<Request>>,
        // Whether to continue on error
        bool,
        // The pending request, which can be polled to retrieve the response
        PendingRequest,
    ),
}

impl std::fmt::Debug for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Element::Raw(_) => write!(f, "Raw"),
            Element::Fragment(_, Some(_), _, _) => write!(f, "Fragment(with alt)"),
            Element::Fragment(_, _, _, _) => write!(f, "Fragment"),
        }
    }
}
