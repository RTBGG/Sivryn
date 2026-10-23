//! A logged-in session: API client, this device's MLS identity and the local
//! encrypted store. Devices and their MLS state survive restarts; logging in
//! again on the same computer reuses the device and its history.

use std::cell::RefCell;
use std::rc::Rc;

use sivryn_client_core::ApiClient;
use sivryn_client_core::protocol::api::{RegisterDeviceRequest, RegisterRequest};
use sivryn_client_core::protocol::model::User;
use sivryn_client_core::protocol::{ChannelId, DeviceId};
use sivryn_crypto::MlsDevice;

use crate::store::{self, AccountId, Store};

/// Key packages the server should hold for this device; each is consumed
/// when someone adds the device to a channel group.
const KEY_PACKAGES: u32 = 50;
/// Top up once fewer than this are left.
const KEY_PACKAGES_LOW: u32 = 20;

#[derive(Clone)]
pub struct Session {
    pub api: ApiClient,
    pub user: User,
    pub device_id: DeviceId,
    pub mls: Rc<RefCell<MlsDevice>>,
    pub store: Rc<Store>,
    /// Channels of guilds that share their history with new members.
    pub history_channels: Rc<RefCell<std::collections::HashSet<ChannelId>>>,
    /// Channels that got new history keys and should look for older
    /// messages they unlock.
    pub history_unlocked: Rc<RefCell<std::collections::HashSet<ChannelId>>>,
}

impl PartialEq for Session {
    fn eq(&self, other: &Self) -> bool {
        self.device_id == other.device_id
    }
}

impl Session {
    /// Logs in (or registers) with a password.
    /// Signs in, or registers a new account.
    pub async fn login(
        instance: &str,
        username: &str,
        password: &str,
        register: bool,
        invite_code: Option<&str>,
    ) -> Result<Self, String> {
        let mut api = ApiClient::new(instance).map_err(|e| e.to_string())?;
        let user = if register {
            let req = RegisterRequest {
                username: username.into(),
                password: password.into(),
                invite_code: invite_code.map(str::to_owned),
            };
            api.register(&req).await
        } else {
            api.login(username, password).await
        }
        .map_err(sign_in_error)?;

        let account = AccountId { instance: api.base_url().to_owned(), user_id: user.id };
        let store = Store::open(&account, api.token()).map_err(|e| e.to_string())?;
        Self::with_device(api, user, store).await
    }

    /// Signs in with the token saved for `account`. `Ok(None)` means the
    /// token is gone or expired and the user has to log in again.
    pub async fn restore(account: &AccountId) -> Result<Option<Self>, String> {
        let Some(token) = store::load_credentials(account).map_err(|e| e.to_string())?.and_then(|c| c.token) else {
            return Ok(None);
        };
        let api = ApiClient::new(&account.instance).map_err(|e| e.to_string())?.with_token(token);
        let store = Store::open(account, None).map_err(|e| e.to_string())?;
        let user = match api.me().await {
            Ok(user) => user,
            Err(e) if e.is_unauthorized() => {
                let _ = store.forget_token();
                return Ok(None);
            }
            Err(e) => return Err(e.to_string()),
        };
        Self::with_device(api, user, store).await.map(Some)
    }

    /// Reuses the stored device if the server still knows it, else registers a new one.
    async fn with_device(api: ApiClient, user: User, store: Store) -> Result<Self, String> {
        let store = Rc::new(store);
        match store.device() {
            Ok(Some((device_id, state))) => match MlsDevice::import(&state) {
                Ok(mls) => {
                    let session = Self {
                        api: api.clone(),
                        user: user.clone(),
                        device_id,
                        mls: Rc::new(RefCell::new(mls)),
                        store: store.clone(),
                        history_channels: Default::default(),
                        history_unlocked: Default::default(),
                    };
                    match session.top_up_key_packages().await {
                        Ok(()) => return Ok(session),
                        Err(e) if e.is_missing() => {
                            tracing::warn!(%device_id, "server no longer knows this device; registering a new one");
                            let _ = store.clear_device();
                        }
                        Err(e) => return Err(e.to_string()),
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "stored device state unreadable; registering a new device");
                    let _ = store.clear_device();
                }
            },
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "could not read stored device"),
        }

        let mls = MlsDevice::new(&user.id.to_string()).map_err(|e| e.to_string())?;
        let key_packages = mls.key_packages(KEY_PACKAGES as usize).map_err(|e| e.to_string())?;
        let device = api
            .register_device(&RegisterDeviceRequest { name: device_name(), key_packages })
            .await
            .map_err(|e| e.to_string())?;
        store.save_device(device.device_id, &mls.export()).map_err(|e| e.to_string())?;
        Ok(Self {
            api,
            user,
            device_id: device.device_id,
            mls: Rc::new(RefCell::new(mls)),
            store,
            history_channels: Default::default(),
            history_unlocked: Default::default(),
        })
    }

    /// Uploads fresh key packages when the server is running low. Their
    /// private keys are saved first, so a crash can't strand a welcome.
    async fn top_up_key_packages(&self) -> Result<(), sivryn_client_core::ClientError> {
        let left = self.api.key_package_count(self.device_id).await?;
        if left >= KEY_PACKAGES_LOW {
            return Ok(());
        }
        let fresh = match self.mls.borrow().key_packages((KEY_PACKAGES - left) as usize) {
            Ok(kps) => kps,
            Err(e) => {
                tracing::warn!(error = %e, "could not create key packages");
                return Ok(());
            }
        };
        self.persist_mls();
        self.api.upload_key_packages(self.device_id, fresh).await
    }

    /// Saves the MLS state. Call after every operation that changes it.
    pub fn persist_mls(&self) {
        if let Err(e) = self.store.save_mls_state(&self.mls.borrow().export()) {
            tracing::error!(error = %e, "could not save MLS state");
        }
    }

    pub fn account(&self) -> AccountId {
        self.store.account().clone()
    }

    /// Ends the session on the server and forgets the token; the device and
    /// history stay, so logging in again picks up where this left off.
    pub async fn logout(&self) {
        if let Err(e) = self.api.logout().await {
            tracing::warn!(error = %e, "server logout failed");
        }
        if let Err(e) = self.store.forget_token() {
            tracing::warn!(error = %e, "could not forget token");
        }
    }
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Desktop".into())
}

/// What went wrong signing in or registering, in words people understand.
fn sign_in_error(e: sivryn_client_core::ClientError) -> String {
    use sivryn_client_core::ClientError;
    let key = match &e {
        ClientError::Api { code, .. } => match code.as_str() {
            "invite_required" => "login.error.invite_required",
            "invalid_invite" => "login.error.invalid_invite",
            "registration_closed" => "login.error.registration_closed",
            "unauthorized" => "login.error.wrong_password",
            "conflict" => "login.error.username_taken",
            _ => return e.to_string(),
        },
        ClientError::Network(_) => "login.error.unreachable",
        _ => return e.to_string(),
    };
    crate::i18n::t(key).to_owned()
}
