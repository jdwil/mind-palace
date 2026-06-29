use axum::{
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::CookieJar;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::app::AppState;

const COOKIE_NAME: &str = "mp_session";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleUserInfo {
    pub email: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub name: String,
    pub exp: usize,
}

#[derive(Deserialize)]
pub struct AuthCallback {
    pub code: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct UserInfoResponse {
    email: String,
    name: Option<String>,
}

pub async fn login(State(state): State<Arc<AppState>>) -> Redirect {
    let url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope=openid%20email%20profile",
        state.config.google_client_id,
        urlencoding::encode(&state.config.redirect_uri),
    );
    Redirect::temporary(&url)
}

pub async fn callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AuthCallback>,
) -> Result<Response, StatusCode> {
    let client = reqwest::Client::new();

    let token_resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", params.code.as_str()),
            ("client_id", state.config.google_client_id.as_str()),
            ("client_secret", state.config.google_client_secret.as_str()),
            ("redirect_uri", state.config.redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?
        .json::<TokenResponse>()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let user_info = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(&token_resp.access_token)
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?
        .json::<UserInfoResponse>()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    if !state.config.allowed_emails.contains(&user_info.email) {
        return Err(StatusCode::FORBIDDEN);
    }

    let exp = chrono::Utc::now().timestamp() as usize + 86400;
    let claims = Claims {
        sub: user_info.email,
        name: user_info.name.unwrap_or_default(),
        exp,
    };
    let token = jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.config.session_secret.as_bytes()),
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let cookie = format!("{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=86400");
    Ok(([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response())
}

pub fn extract_claims(jar: &CookieJar, secret: &str) -> Option<Claims> {
    let cookie = jar.get(COOKIE_NAME)?;
    let token_data = jsonwebtoken::decode::<Claims>(
        cookie.value(),
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .ok()?;
    Some(token_data.claims)
}

mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(b as char);
                }
                _ => {
                    result.push_str(&format!("%{:02X}", b));
                }
            }
        }
        result
    }
}
