use fastly::{http::request::PendingRequest, Request};

/// A section of the pending response, either raw XML data or a pending fragment request.
pub enum Element {
    Raw(Vec<u8>),
    Fragment(Request, PendingRequest),
}

impl std::fmt::Debug for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Element::Raw(_) => write!(f, "Raw"),
            Element::Fragment(_, _) => write!(f, "Fragment"),
        }
    }
}
