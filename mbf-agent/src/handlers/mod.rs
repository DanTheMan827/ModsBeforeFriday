//! Responsible for routing requests from the frontend to `mbf-core`.

use anyhow::Result;

use crate::models::response::Response;
use mbf_core::models::request::RequestEnum;

pub fn handle_request(request: RequestEnum) -> Result<Response> {
    mbf_core::runtime::handle_request(&mut crate::host::AgentHost::new(), request)
}
