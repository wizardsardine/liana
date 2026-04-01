use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::service::Interceptor;

use crate::services::connect::client::auth::AccessTokenResponse;

/// gRPC metadata interceptor that attaches the JWT bearer token.
///
/// Shares the same `Arc<RwLock<AccessTokenResponse>>` as the REST `BackendClient`,
/// so token refreshes are automatically picked up by gRPC calls.
#[derive(Debug, Clone)]
pub struct AuthInterceptor {
    tokens: Arc<RwLock<AccessTokenResponse>>,
}

impl AuthInterceptor {
    pub fn new(tokens: Arc<RwLock<AccessTokenResponse>>) -> Self {
        Self { tokens }
    }
}

impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        // `blocking_read()` is safe here because the interceptor runs in a
        // sync context within the tonic transport layer; the lock is held
        // only for the duration of reading the access_token string.
        let token = self.tokens.blocking_read();
        let bearer = format!("Bearer {}", token.access_token);
        req.metadata_mut().insert(
            "authorization",
            bearer
                .parse()
                .map_err(|_| tonic::Status::internal("invalid authorization header value"))?,
        );
        Ok(req)
    }
}
