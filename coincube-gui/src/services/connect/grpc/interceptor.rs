use tonic::service::Interceptor;

/// gRPC metadata interceptor that attaches a JWT bearer token.
///
/// Holds a pre-formatted `Bearer <token>` string. The interceptor runs synchronously
/// on tokio worker threads, so it must not touch `tokio::sync` primitives. Callers
/// read the current token asynchronously before constructing the interceptor — for
/// the realtime stream this happens on every reconnect, which is sufficient since
/// the interceptor only runs once per stream.
#[derive(Debug, Clone)]
pub struct AuthInterceptor {
    bearer: String,
}

impl AuthInterceptor {
    pub fn new(access_token: &str) -> Self {
        Self {
            bearer: format!("Bearer {}", access_token),
        }
    }
}

impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        req.metadata_mut().insert(
            "authorization",
            self.bearer
                .parse()
                .map_err(|_| tonic::Status::internal("invalid authorization header value"))?,
        );
        Ok(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attaches_bearer_token() {
        let mut interceptor = AuthInterceptor::new("test-token-abc");
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
        let mut interceptor = AuthInterceptor::new("invalid\ntoken");
        let req = tonic::Request::new(());
        let err = interceptor
            .call(req)
            .expect_err("should reject invalid token");
        assert_eq!(err.code(), tonic::Code::Internal);
    }
}
