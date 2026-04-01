use tonic::transport::Channel;

use super::connect_v1::{
    session_service_client::SessionServiceClient, CancelSigningSessionRequest,
    CreateSigningSessionRequest, GetSigningSessionRequest, GetSigningSessionResponse,
    ListPendingSessionsResponse, SigningSession,
};
use super::interceptor::AuthInterceptor;

type InterceptedSessionClient =
    SessionServiceClient<tonic::service::interceptor::InterceptedService<Channel, AuthInterceptor>>;

/// Client wrapper for the Connect SessionService.
#[derive(Debug, Clone)]
pub struct GrpcSessionClient {
    inner: InterceptedSessionClient,
}

impl GrpcSessionClient {
    pub fn new(channel: Channel, interceptor: AuthInterceptor) -> Self {
        Self {
            inner: SessionServiceClient::with_interceptor(channel, interceptor),
        }
    }

    /// Create a new PSBT signing session.
    pub async fn create_signing_session(
        &mut self,
        request: CreateSigningSessionRequest,
    ) -> Result<SigningSession, tonic::Status> {
        self.inner
            .create_signing_session(request)
            .await
            .map(|r| r.into_inner().session.unwrap_or_default())
    }

    /// Fetch the current state of a signing session.
    pub async fn get_signing_session(
        &mut self,
        session_id: String,
    ) -> Result<GetSigningSessionResponse, tonic::Status> {
        let request = GetSigningSessionRequest { session_id };
        self.inner
            .get_signing_session(request)
            .await
            .map(|r| r.into_inner())
    }

    /// List all pending signing sessions for the authenticated user.
    pub async fn list_pending_sessions(
        &mut self,
    ) -> Result<ListPendingSessionsResponse, tonic::Status> {
        self.inner
            .list_pending_sessions(super::connect_v1::ListPendingSessionsRequest {})
            .await
            .map(|r| r.into_inner())
    }

    /// Cancel an in-flight signing session.
    /// Only allowed from: PENDING, DELIVERED, VIEWED, APPROVED.
    pub async fn cancel_signing_session(
        &mut self,
        session_id: String,
        reason: String,
    ) -> Result<(), tonic::Status> {
        let request = CancelSigningSessionRequest { session_id, reason };
        self.inner.cancel_signing_session(request).await.map(|_| ())
    }
}
