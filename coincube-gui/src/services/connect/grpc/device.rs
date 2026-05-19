use tonic::transport::Channel;

use super::connect_v1::{
    device_service_client::DeviceServiceClient, DevicePlatform, RegisterDeviceRequest,
    RegisterDeviceResponse, UpdatePushTokenRequest,
};
use super::interceptor::AuthInterceptor;

type InterceptedDeviceClient =
    DeviceServiceClient<tonic::service::interceptor::InterceptedService<Channel, AuthInterceptor>>;

/// Client wrapper for the Connect DeviceService.
#[derive(Debug, Clone)]
pub struct GrpcDeviceClient {
    inner: InterceptedDeviceClient,
}

impl GrpcDeviceClient {
    pub fn new(channel: Channel, interceptor: AuthInterceptor) -> Self {
        Self {
            inner: DeviceServiceClient::with_interceptor(channel, interceptor),
        }
    }

    /// Register this desktop as a device. Returns the server-assigned device ID.
    /// Idempotent — the server may return an existing device if already registered.
    pub async fn register_device(
        &mut self,
        device_name: String,
        app_version: String,
        os_version: String,
    ) -> Result<RegisterDeviceResponse, tonic::Status> {
        let request = RegisterDeviceRequest {
            device_name,
            platform: DevicePlatform::Desktop as i32,
            push_token: String::new(), // Desktop doesn't use push notifications
            app_version,
            os_version,
            device_pubkey: String::new(), // Optional, for v2 assertions
            capabilities: vec!["create_session".to_string(), "cancel_session".to_string()],
        };
        self.inner
            .register_device(request)
            .await
            .map(|r| r.into_inner())
    }

    /// Update the push token for a device (no-op for desktop, but available for completeness).
    pub async fn update_push_token(
        &mut self,
        device_id: String,
        push_token: String,
    ) -> Result<(), tonic::Status> {
        let request = UpdatePushTokenRequest {
            device_id,
            push_token,
        };
        self.inner.update_push_token(request).await.map(|_| ())
    }
}
