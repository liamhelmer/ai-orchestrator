use serde::Deserialize;
use worker::*;

use crate::models::User;

/// Row returned from D1 session query
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SessionRow {
    session_id: String,
    user_id: String,
    expires_at: String,
    github_id: i64,
    github_login: String,
    email: Option<String>,
}

/// Authentication middleware for protected routes
pub struct AuthMiddleware;

impl AuthMiddleware {
    /// Validate session and return user if authenticated
    pub async fn get_user(req: &Request, env: &Env) -> Result<Option<User>> {
        let session_id = match Self::get_session_cookie(req) {
            Some(id) => id,
            None => return Ok(None),
        };

        // Look up session in D1
        let db = env.d1("DB")?;
        let result = db.prepare(
            "SELECT s.id as session_id, s.user_id, s.expires_at,
                    u.github_id, u.github_login, u.email
             FROM sessions s
             JOIN users u ON s.user_id = u.id
             WHERE s.id = ?1 AND s.expires_at > datetime('now')"
        )
        .bind(&[session_id.into()])?
        .first::<SessionRow>(None)
        .await?;

        match result {
            Some(row) => {
                // Session is valid, create user
                Ok(Some(User::from_db(
                    row.user_id,
                    row.github_id,
                    row.github_login,
                    row.email,
                )))
            }
            None => Ok(None),
        }
    }

    /// Require authentication, returning error response if not authenticated
    pub async fn require_auth(
        req: &Request,
        env: &Env,
    ) -> Result<std::result::Result<User, Response>> {
        match Self::get_user(req, env).await? {
            Some(user) => Ok(Ok(user)),
            None => {
                // Return redirect to login
                let headers = Headers::new();
                headers.set("Location", "/auth/github")?;
                let response = Response::empty()?.with_status(302).with_headers(headers);
                Ok(Err(response))
            }
        }
    }

    fn get_session_cookie(req: &Request) -> Option<String> {
        let cookie_header = req.headers().get("Cookie").ok()??;
        for part in cookie_header.split(';') {
            let part = part.trim();
            if let Some(value) = part.strip_prefix("session=") {
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        None
    }
}
