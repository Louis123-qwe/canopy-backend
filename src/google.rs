use axum::{
    extract::{Json as ExtractJson, State},
    http::StatusCode,
    Json,
};
use bcrypt::{hash, DEFAULT_COST};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{issue_token, UserPublic};
use crate::db::{find_user_by_email, UserRow};
use crate::Db;

#[derive(Debug, Deserialize)]
pub struct GoogleLoginRequest {
    pub credential: String,
}

#[derive(Debug, Deserialize)]
struct GoogleClaims {
    sub: String,
    email: String,
    #[serde(default)]
    email_verified: Value,
    given_name: Option<String>,
    family_name: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleJwk {
    kid: String,
    n: String,
    e: String,
}

#[derive(Debug, Deserialize)]
struct GoogleJwks {
    keys: Vec<GoogleJwk>,
}

fn error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "status": "error", "message": message })))
}

fn is_verified(value: &Value) -> bool {
    value.as_bool() == Some(true) || value.as_str() == Some("true")
}

async fn verify_google_token(id_token: &str) -> Result<GoogleClaims, String> {
    let client_id = std::env::var("GOOGLE_CLIENT_ID")
        .map_err(|_| "Google sign-in is not configured".to_string())?;

    let header = decode_header(id_token).map_err(|_| "Invalid Google token".to_string())?;
    let kid = header.kid.ok_or_else(|| "Invalid Google token".to_string())?;

    let jwks: GoogleJwks = reqwest::Client::new()
        .get("https://www.googleapis.com/oauth2/v3/certs")
        .send()
        .await
        .map_err(|e| format!("Could not reach Google: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Could not read Google's signing keys: {e}"))?;

    let jwk = jwks
        .keys
        .into_iter()
        .find(|k| k.kid == kid)
        .ok_or_else(|| "Unknown Google signing key".to_string())?;

    let key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
        .map_err(|_| "Invalid Google signing key".to_string())?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[client_id]);
    validation.set_issuer(&["https://accounts.google.com", "accounts.google.com"]);

    let data = decode::<GoogleClaims>(id_token, &key, &validation).map_err(|e| {
        eprintln!("Google token rejected: {e}");
        "Invalid or expired Google token".to_string()
    })?;

    Ok(data.claims)
}

fn google_names(claims: &GoogleClaims, email: &str) -> (String, String) {
    let given = claims.given_name.clone().unwrap_or_default();
    let family = claims.family_name.clone().unwrap_or_default();

    if !given.trim().is_empty() {
        return (given.trim().to_string(), family.trim().to_string());
    }

    if let Some(name) = &claims.name {
        let mut parts = name.split_whitespace();
        if let Some(first) = parts.next() {
            let rest: Vec<&str> = parts.collect();
            return (first.to_string(), rest.join(" "));
        }
    }

    (email.split('@').next().unwrap_or("User").to_string(), String::new())
}

async fn find_user_by_google_id(db: &Db, google_id: &str) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        r#"
        SELECT id, first_name, last_name, email, password_hash
        FROM users
        WHERE google_id = $1
        "#,
    )
    .bind(google_id)
    .fetch_optional(db)
    .await
}

async fn link_google_id(db: &Db, user_id: Uuid, google_id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(r#"UPDATE users SET google_id = $1 WHERE id = $2 AND google_id IS NULL"#)
        .bind(google_id)
        .bind(user_id)
        .execute(db)
        .await?;

    Ok(result.rows_affected() > 0)
}

async fn insert_google_user(
    db: &Db,
    id: Uuid,
    first_name: &str,
    last_name: &str,
    email: &str,
    password_hash: &str,
    google_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO users (id, first_name, last_name, email, password_hash, google_id, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, now())
        "#,
    )
    .bind(id)
    .bind(first_name)
    .bind(last_name)
    .bind(email)
    .bind(password_hash)
    .bind(google_id)
    .execute(db)
    .await?;

    Ok(())
}

pub async fn google_login(
    State(db): State<Db>,
    ExtractJson(payload): ExtractJson<GoogleLoginRequest>,
) -> (StatusCode, Json<Value>) {
    let claims = match verify_google_token(&payload.credential).await {
        Ok(c) => c,
        Err(msg) => return error(StatusCode::UNAUTHORIZED, &msg),
    };

    if !is_verified(&claims.email_verified) {
        return error(StatusCode::UNAUTHORIZED, "Your Google email address is not verified");
    }

    let email = claims.email.trim().to_lowercase();

    let existing = match find_user_by_google_id(&db, &claims.sub).await {
        Ok(Some(row)) => Some(row),
        Ok(None) => match find_user_by_email(&db, &email).await {
            Ok(Some(row)) => match link_google_id(&db, row.id, &claims.sub).await {
                Ok(true) => Some(row),
                Ok(false) => {
                    return error(
                        StatusCode::CONFLICT,
                        "This email is already linked to a different Google account",
                    );
                }
                Err(e) => {
                    eprintln!("DB error linking Google account: {e}");
                    return error(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong. Please try again.");
                }
            },
            Ok(None) => None,
            Err(e) => {
                eprintln!("DB error finding user by email: {e}");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong. Please try again.");
            }
        },
        Err(e) => {
            eprintln!("DB error finding user by Google id: {e}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong. Please try again.");
        }
    };

    let (user_id, first_name, last_name, user_email, status) = match existing {
        Some(row) => (row.id, row.first_name, row.last_name, row.email, StatusCode::OK),
        None => {
            let (first_name, last_name) = google_names(&claims, &email);
            let user_id = Uuid::new_v4();
            let unusable_password = format!("{}{}", Uuid::new_v4(), Uuid::new_v4());

            let password_hash = match hash(&unusable_password, DEFAULT_COST) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("Hashing error: {e}");
                    return error(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong. Please try again.");
                }
            };

            if let Err(e) = insert_google_user(
                &db,
                user_id,
                &first_name,
                &last_name,
                &email,
                &password_hash,
                &claims.sub,
            )
            .await
            {
                eprintln!("DB error inserting Google user: {e}");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "Could not create account. Please try again.");
            }

            (user_id, first_name, last_name, email.clone(), StatusCode::CREATED)
        }
    };

    let token = match issue_token(&user_id.to_string(), &user_email) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("JWT error: {e}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong. Please try again.");
        }
    };

    let user = UserPublic {
        id: user_id.to_string(),
        first_name,
        last_name,
        email: user_email,
    };

    (status, Json(json!({ "status": "ok", "user": user, "token": token })))
}
