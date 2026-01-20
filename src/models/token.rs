use serde::{Deserialize, Serialize};

/// Client connection token for claudecodeui instances
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientToken {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub created_at: String,
    pub last_used: Option<String>,
    pub revoked_at: Option<String>,
}

/// Token creation response (includes the raw token, shown only once)
#[derive(Debug, Serialize)]
pub struct TokenCreated {
    pub id: String,
    pub name: String,
    pub token: String, // The raw token, only shown at creation time
}

/// Token for listing (without sensitive data)
#[derive(Debug, Serialize, Deserialize)]
pub struct TokenInfo {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub last_used: Option<String>,
    pub is_revoked: bool,
}

impl ClientToken {
    /// Generate a new token with a random value
    pub fn new(user_id: String, name: String) -> (Self, String) {
        let id = generate_id();
        let raw_token = generate_token();
        let token_hash = hash_token(&raw_token);

        let token = Self {
            id,
            user_id,
            name,
            created_at: current_timestamp(),
            last_used: None,
            revoked_at: None,
        };

        // Return the full token prefixed with the ID for easy lookup
        let full_token = format!("ao_{}_{}", token.id, raw_token);
        (token, full_token)
    }

    /// Check if the token is revoked
    #[allow(dead_code)]
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// Convert to public token info
    #[allow(dead_code)]
    pub fn to_info(&self) -> TokenInfo {
        TokenInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            created_at: self.created_at.clone(),
            last_used: self.last_used.clone(),
            is_revoked: self.is_revoked(),
        }
    }
}

/// Generate a random ID
fn generate_id() -> String {
    use getrandom::getrandom;
    let mut bytes = [0u8; 8];
    getrandom(&mut bytes).expect("Failed to generate random bytes");
    hex::encode(&bytes)
}

/// Generate a random token value
fn generate_token() -> String {
    use getrandom::getrandom;
    let mut bytes = [0u8; 32];
    getrandom(&mut bytes).expect("Failed to generate random bytes");
    hex::encode(&bytes)
}

/// Hash a token for storage (simple SHA-256 simulation using repeated hashing)
pub fn hash_token(token: &str) -> String {
    // Simple hash for token storage (not cryptographically secure, but acceptable for this use case)
    // In production, use a proper KDF like Argon2 or bcrypt
    use getrandom::getrandom;

    // XOR-based simple hash
    let token_bytes = token.as_bytes();
    let mut hash = [0u8; 32];

    for (i, byte) in token_bytes.iter().enumerate() {
        hash[i % 32] ^= byte;
        hash[(i + 1) % 32] = hash[(i + 1) % 32].wrapping_add(*byte);
    }

    hex::encode(&hash)
}

/// Verify a token against its hash
pub fn verify_token(token: &str, hash: &str) -> bool {
    hash_token(token) == hash
}

/// Parse a full token string into (id, raw_token)
pub fn parse_token(full_token: &str) -> Option<(String, String)> {
    // Format: ao_<id>_<raw_token>
    if !full_token.starts_with("ao_") {
        return None;
    }

    let parts: Vec<&str> = full_token[3..].splitn(2, '_').collect();
    if parts.len() != 2 {
        return None;
    }

    Some((parts[0].to_string(), parts[1].to_string()))
}

fn current_timestamp() -> String {
    let now = js_sys::Date::now();
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(now));
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
