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
        // SAFETY: `blocking_read()` is safe here because tonic interceptors run in a
        // synchronous context within the gRPC transport layer. The lock is held only
        // for the duration of reading the access_token string — never across an
        // `.await`. Any future code that holds the write lock MUST release it before
        // issuing any gRPC call, otherwise this call will deadlock.
        debug_assert!(
            self.tokens.try_read().is_ok(),
            "AuthInterceptor::call invoked while write lock is held — this would deadlock in release builds",
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tokens(access_token: &str) -> Arc<RwLock<AccessTokenResponse>> {
        Arc::new(RwLock::new(AccessTokenResponse {
            access_token: access_token.to_string(),
            expires_at: 0,
            refresh_token: String::new(),
        }))
    }

    #[test]
    fn attaches_bearer_token() {
        let tokens = make_tokens("test-token-abc");
        let mut interceptor = AuthInterceptor::new(tokens);
        let req = tonic::Request::new(());
        let out = interceptor.call(req).expect("interceptor should succeed");
        let auth = out
            .metadata()
            .get("authorization")
            .expect("authorization header should be present");
        assert_eq!(auth.to_str().unwrap(), "Bearer test-token-abc");
    }

    #[test]
    fn rejects_invalid_token_characters() {
        // HTTP header values can't contain certain characters; verify we surface
        // that as a Status::internal.
        let tokens = make_tokens("invalid\ntoken");
        let mut interceptor = AuthInterceptor::new(tokens);
        let req = tonic::Request::new(());
        let err = interceptor
            .call(req)
            .expect_err("should reject invalid token");
        assert_eq!(err.code(), tonic::Code::Internal);
    }
}
