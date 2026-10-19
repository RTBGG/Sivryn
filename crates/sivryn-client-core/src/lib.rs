//! UI-independent client logic shared by every Sivryn frontend.

mod api;
mod gateway;

pub use api::{ApiClient, ClientError};
pub use gateway::{GatewayConnection, connect_gateway, install_crypto_provider};
pub use sivryn_protocol as protocol;
