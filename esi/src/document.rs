use std::collections::VecDeque;

use crate::Result;
use fastly::{http::request::PendingRequest, Request};

pub struct Fragment {
    // Metadata of the request
    pub(crate) request: Request,
    // An optional alternate request to send if the original request fails
    pub(crate) alt: Option<Result<Request>>,
    // Whether to continue on error
    pub(crate) continue_on_error: bool,
    // The pending request, which can be polled to retrieve the response
    pub(crate) pending_request: PendingRequest,
}

/// `Task` is combining raw data and an include fragment for both `attempt` and `except` arms
/// before copied to `Element::Raw` when all the `Fragment`s are processed.
#[derive(Default)]
pub struct Task {
    pub raw: Vec<u8>,
    pub include: VecDeque<Fragment>,
    pub task_status: PollTaskState,
}

impl Task {
    pub fn new() -> Self {
        Self::default()
    }
}

/// A section of the pending response, either raw XML data or a pending fragment request.
pub enum Element {
    Raw(Vec<u8>),
    Include(Fragment),
    Try {
        // active_task: TryTasks,
        except_task: Task,
        attempt_task: Task,
    },
}

#[derive(PartialEq, Clone)]
pub enum PollTaskState {
    Failed(String, u16),
    Pending,
    Succeeded,
}
impl Default for PollTaskState {
    fn default() -> Self {
        Self::Pending
    }
}

impl std::fmt::Debug for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Raw(_) => write!(f, "Raw"),
            Self::Include(Fragment { alt: Some(_), .. }) => {
                write!(f, "Incldude Fragment(with alt)")
            }
            Self::Include(Fragment { .. }) => write!(f, "Include Fragment"),
            Self::Try { .. } => write!(f, "Try"),
        }
    }
}
