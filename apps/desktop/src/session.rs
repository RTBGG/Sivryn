//! A logged-in session: API client, this device's MLS identity and the
//! gateway event pump.

use std::cell::RefCell;
use std::rc::Rc;

use sivryn_client_core::ApiClient;
use sivryn_client_core::protocol::DeviceId;
use sivryn_client_core::protocol::api::RegisterDeviceRequest;
use sivryn_client_core::protocol::model::User;
use sivryn_crypto::MlsDevice;

/// Key packages uploaded at login; each is consumed when someone adds this
/// device to a channel group.
const KEY_PACKAGES: usize = 50;

#[derive(Clone)]
pub struct Session {
    pub api: ApiClient,
    pub user: User,
    pub device_id: DeviceId,
    pub mls: Rc<RefCell<MlsDevice>>,
}

impl PartialEq for Session {
    fn eq(&self, other: &Self) -> bool {
        self.device_id == other.device_id
    }
}

impl Session {
    /// Logs in (or registers) and sets up a fresh MLS device.
    ///
    /// TODO: persist the device and its MLS state, and store the session token
    /// in the OS keychain, instead of creating a new device on every start.
    pub async fn start(instance: &str, username: &str, password: &str, register: bool) -> Result<Self, String> {
        let mut api = ApiClient::new(instance).map_err(|e| e.to_string())?;
        let user = if register {
            let req = sivryn_client_core::protocol::api::RegisterRequest {
                username: username.into(),
                password: password.into(),
                invite_code: None,
            };
            api.register(&req).await
        } else {
            api.login(username, password).await
        }
        .map_err(|e| e.to_string())?;

        let mls = MlsDevice::new(&user.id.to_string()).map_err(|e| e.to_string())?;
        let key_packages = mls.key_packages(KEY_PACKAGES).map_err(|e| e.to_string())?;
        let device = api
            .register_device(&RegisterDeviceRequest { name: device_name(), key_packages })
            .await
            .map_err(|e| e.to_string())?;

        Ok(Self { api, user, device_id: device.device_id, mls: Rc::new(RefCell::new(mls)) })
    }
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Desktop".into())
}
