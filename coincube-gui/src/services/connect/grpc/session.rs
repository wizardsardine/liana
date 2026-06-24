use std::convert::TryFrom;

use tonic::transport::Channel;

use super::connect_v1::{
    session_service_client::SessionServiceClient, CancelSigningSessionRequest,
    CreateDecryptRequestRequest, CreateSigningSessionRequest, DecryptRequestStatus,
    GetDecryptResultRequest, GetSigningSessionRequest, GetSigningSessionResponse,
    ListPendingSessionsResponse, ResolveSignersRequest, ResolveSignersResponse, SigningSession,
};
use super::interceptor::AuthInterceptor;

/// Terminal-or-pending outcome of an inheritance decrypt-relay request,
/// translated from the proto `DecryptRequestStatus` so callers don't depend on
/// generated types. `Completed` carries the opaque `wrapped_shared_key` to
/// unwrap (SPEC-ecies-v1 §4b).
#[derive(Debug, Clone)]
pub enum DecryptOutcome {
    /// Still awaiting the heir's Keychain approval — poll again.
    Pending,
    /// Keychain approved; carries the ECIES-wrapped key to unwrap locally.
    Completed(Vec<u8>),
    /// Keychain declined (approval denied or the heir is under duress).
    Rejected,
    /// The request TTL elapsed without a Keychain answer.
    Expired,
}

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
            .and_then(|r| {
                r.into_inner().session.ok_or_else(|| {
                    tonic::Status::internal("CreateSigningSession response missing session field")
                })
            })
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

    /// Resolve the live signer targets for a vault.
    ///
    /// For each Keychain signer attached to the vault, the API looks up the
    /// owner user's primary `SignerDevice` so the desktop can address
    /// `CreateSigningSession`. Returns `targets` for successfully resolved
    /// signers and `unresolved` for any whose owner has no usable device
    /// (no device registered, all devices revoked, or owner unknown).
    pub async fn resolve_signers(
        &mut self,
        vault_id: String,
    ) -> Result<ResolveSignersResponse, tonic::Status> {
        let request = ResolveSignersRequest { vault_id };
        self.inner
            .resolve_signers(request)
            .await
            .map(|r| r.into_inner())
    }

    /// Heir desktop — inheritance decrypt relay, step 1. Brokers a decrypt of
    /// the envelope addressed to us for (`cube_id`, `artifact_kind`); the server
    /// applies the recovery gate and notifies the heir's Keychain, which (after
    /// biometric/PIN approval) ECIES-**wraps** the derived key to
    /// `transport_pubkey` (compressed SEC1) — the server never sees the key.
    /// Idempotent on `request_id`.
    pub async fn create_decrypt_request(
        &mut self,
        request_id: String,
        cube_id: String,
        artifact_kind: String,
        transport_pubkey: Vec<u8>,
    ) -> Result<(), tonic::Status> {
        let request = CreateDecryptRequestRequest {
            request_id,
            cube_id,
            artifact_kind,
            desktop_transport_pubkey: transport_pubkey,
        };
        self.inner.create_decrypt_request(request).await.map(|_| ())
    }

    /// Heir desktop — inheritance decrypt relay, step 4 (poll). Fetches the
    /// current [`DecryptOutcome`] of a request; the companion to the best-effort
    /// `decrypt_result` stream push. On `Completed`, the wrapped key is unwrapped
    /// locally with the per-recovery transport private key (SPEC §4b).
    pub async fn get_decrypt_result(
        &mut self,
        request_id: String,
    ) -> Result<DecryptOutcome, tonic::Status> {
        let request = GetDecryptResultRequest { request_id };
        let resp = self.inner.get_decrypt_result(request).await?.into_inner();
        let status = DecryptRequestStatus::try_from(resp.status)
            .map_err(|_| tonic::Status::internal("GetDecryptResult returned an unknown status"))?;
        Ok(match status {
            DecryptRequestStatus::Completed => DecryptOutcome::Completed(resp.wrapped_shared_key),
            DecryptRequestStatus::Rejected => DecryptOutcome::Rejected,
            DecryptRequestStatus::Expired => DecryptOutcome::Expired,
            DecryptRequestStatus::Pending | DecryptRequestStatus::Unspecified => {
                DecryptOutcome::Pending
            }
        })
    }
}
