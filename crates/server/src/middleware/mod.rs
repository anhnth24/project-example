//! HTTP middleware: request ID, CORS, trusted-proxy client IP, and in-process rate limits.

mod client_ip;
mod cors;
mod rate_limit;
mod request_id;

pub use client_ip::{resolve_client_ip, ClientIpError};
pub use cors::cors_layer;
pub use rate_limit::{
    rate_limit_middleware, EndpointClass, InMemoryRateLimiter, RateLimitDecision,
};
pub use request_id::{request_id_middleware, RequestId, ResolvedRequestId, REQUEST_ID_HEADER};
