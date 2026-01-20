use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientStatus {
    Idle,
    Active,
    Busy,
    Disconnected,
}

impl Default for ClientStatus {
    fn default() -> Self {
        Self::Idle
    }
}

impl std::fmt::Display for ClientStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Active => write!(f, "active"),
            Self::Busy => write!(f, "busy"),
            Self::Disconnected => write!(f, "disconnected"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientMetadata {
    pub hostname: String,
    pub project: String,
    #[serde(default)]
    pub status: ClientStatus,
    pub last_activity: Option<String>,
    /// Optional HTTP callback URL for direct proxying (e.g., http://localhost:3010 or https://tunnel.ngrok.io)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Client {
    pub id: String,
    pub user_id: String,
    pub metadata: ClientMetadata,
    pub connected_at: String,
    pub last_seen: String,
}

impl Client {
    pub fn new(id: String, user_id: String, metadata: ClientMetadata) -> Self {
        let now = current_timestamp();
        Self {
            id,
            user_id,
            metadata,
            connected_at: now.clone(),
            last_seen: now,
        }
    }

    pub fn update_last_seen(&mut self) {
        self.last_seen = current_timestamp();
    }

    pub fn update_status(&mut self, status: ClientStatus) {
        self.metadata.status = status;
        self.metadata.last_activity = Some(current_timestamp());
    }
}

fn current_timestamp() -> String {
    let now = js_sys::Date::now();
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(now));
    date.to_iso_string().as_string().unwrap_or_default()
}

mod js_sys {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        pub type Date;

        #[wasm_bindgen(constructor)]
        pub fn new(value: &JsValue) -> Date;

        #[wasm_bindgen(static_method_of = Date)]
        pub fn now() -> f64;

        #[wasm_bindgen(method, js_name = toISOString)]
        pub fn to_iso_string(this: &Date) -> JsString;
    }

    #[wasm_bindgen]
    extern "C" {
        pub type JsString;

        #[wasm_bindgen(method, js_name = toString)]
        pub fn as_string(this: &JsString) -> Option<String>;
    }
}
