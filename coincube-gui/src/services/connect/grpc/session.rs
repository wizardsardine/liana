use tonic::transport::Channel;

use super::connect_v1::{
    session_service_client::SessionServiceClient, CancelSigningSessionRequest,
    CreateSigningSessionRequest, DecryptInheritanceEnvelopeRequest, GetSigningSessionRequest,
    GetSigningSessionResponse, ListPendingSessionsResponse, ResolveSignersRequest,
    ResolveSignersResponse, SigningSession,
};
use super::interceptor::AuthInterceptor;

/// secp256k1 ECDH + HKDF-SHA256 yield a 32-byte symmetric key; the desktop
/// rejects anything else from the relayed Keychain response (a malformed key
/// would otherwise surface as an opaque AES-GCM open failure).
const INHERITANCE_SHARED_KEY_LEN: usize = 32;

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

    /// Heir-side inheritance recovery: ask the heir's Keychain (via the API
    /// relay) to ECDH-decrypt one ECIES envelope and return the derived
    /// symmetric key `K`. Keychain approves with biometric/PIN and never
    /// returns the recovery private key or the seed/descriptor plaintext — the
    /// desktop completes the AES-256-GCM open with `K`. `purpose` pins the
    /// scheme so a Keychain that doesn't implement it can refuse.
    ///
    /// `ephemeral_pubkey` is the 33-byte compressed point and `derivation` the
    /// non-hardened child path, both taken verbatim from the envelope.
    pub async fn decrypt_inheritance_envelope(
        &mut self,
        cube_id: String,
        ephemeral_pubkey: Vec<u8>,
        derivation: String,
    ) -> Result<Vec<u8>, tonic::Status> {
        let request = DecryptInheritanceEnvelopeRequest {
            cube_id,
            ephemeral_pubkey,
            derivation,
            purpose: crate::services::inheritance::SCHEME.to_string(),
        };
        let resp = self
            .inner
            .decrypt_inheritance_envelope(request)
            .await?
            .into_inner();
        if resp.shared_key.len() != INHERITANCE_SHARED_KEY_LEN {
            return Err(tonic::Status::internal(
                "DecryptInheritanceEnvelope returned a shared key of the wrong length",
            ));
        }
        Ok(resp.shared_key)
    }
}
