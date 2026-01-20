use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub github_id: i64,
    pub github_login: String,
    pub email: Option<String>,
    pub created_at: String,
    pub last_login: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub expires_at: String,
    pub created_at: String,
}

impl User {
    pub fn new(github_id: i64, github_login: String, email: Option<String>) -> Self {
        Self {
            id: generate_id(),
            github_id,
            github_login,
            email,
            created_at: current_timestamp(),
            last_login: Some(current_timestamp()),
        }
    }

    /// Create user from D1 database row
    pub fn from_db(
        id: String,
        github_id: i64,
        github_login: String,
        email: Option<String>,
    ) -> Self {
        Self {
            id,
            github_id,
            github_login,
            email,
            created_at: String::new(), // Not needed when loaded from DB
            last_login: None,
        }
    }
}

impl Session {
    pub fn new(user_id: String, duration_hours: u64) -> Self {
        Self {
            id: generate_id(),
            user_id,
            expires_at: future_timestamp(duration_hours),
            created_at: current_timestamp(),
        }
    }

    pub fn is_expired(&self) -> bool {
        // Simple string comparison works for ISO 8601 timestamps
        self.expires_at < current_timestamp()
    }
}

fn generate_id() -> String {
    use getrandom::getrandom;
    let mut bytes = [0u8; 16];
    getrandom(&mut bytes).expect("Failed to generate random bytes");
    hex::encode(&bytes)
}

fn current_timestamp() -> String {
    // In WASM, we use js_sys for time
    let now = js_sys::Date::now();
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(now));
    date.to_iso_string().as_string().unwrap_or_default()
}

fn future_timestamp(hours: u64) -> String {
    let now = js_sys::Date::now();
    let future = now + (hours as f64 * 3600.0 * 1000.0);
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(future));
    date.to_iso_string().as_string().unwrap_or_default()
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
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
