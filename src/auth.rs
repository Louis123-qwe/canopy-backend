// ============================================================
// CANOPY — Auth module (signup / login)
// Matches the existing Axum + Postgres patterns used elsewhere
// in main.rs (State<Db>, ExtractJson, Json<Value> responses).
// ============================================================

use axum::{
    async_trait,
    extract::{FromRequestParts, Json as ExtractJson, State},
    http::{request::Parts, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

// bcrypt handles hashing + verification together, and includes
// its own per-password salt — no separate salt column needed.
use bcrypt::{hash, verify, DEFAULT_COST};

// jsonwebtoken for issuing AND verifying session tokens.
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};

// Swap this for your real Postgres pool type (e.g. sqlx::PgPool)
// — kept generic here so it drops into whatever `Db` type your
// main.rs already defines and passes via `.with_state(db)`.
use crate::Db;

// ---------- Request / response shapes ----------

#[derive(Debug, Deserialize)]
pub struct SignupRequest {
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct UserPublic {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String, // user id
    email: String,
    exp: usize,
}

// ---------- Validation ----------

/// Mirrors the frontend's password_rules exactly, so a password
/// that passes the UI checklist always passes here too — and one
/// that's rejected here would have been caught client-side first.
fn validate_password(pw: &str) -> Result<(), &'static str> {
    if pw.len() < 8 {
        return Err("Password must be at least 8 characters");
    }
    if !pw.chars().any(|c| c.is_ascii_uppercase()) {
        return Err("Password must include an uppercase letter");
    }
    if !pw.chars().any(|c| c.is_ascii_digit()) {
        return Err("Password must include a number");
    }
    if !pw.chars().any(|c| !c.is_ascii_alphanumeric()) {
        return Err("Password must include a special character");
    }
    Ok(())
}

fn validate_email(email: &str) -> Result<(), &'static str> {
    // Deliberately simple — real validation happens by actually
    // sending a verification email, not by regex gymnastics here.
    if !email.contains('@') || !email.contains('.') || email.len() < 5 {
        return Err("Enter a valid email address");
    }
    Ok(())
}

fn error_response(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "status": "error", "message": message })))
}

// ---------- JWT issuing ----------

pub fn issue_token(user_id: &str, email: &str) -> Result<String, jsonwebtoken::errors::Error> {
    // In production, load this from an environment variable —
    // never hardcode a real secret. Treat this the same way you
    // treated firebase.json: never paste the real value in chat.
    let secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-only-insecure-secret".into());

    let expiration = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::days(7))
        .expect("valid timestamp")
        .timestamp() as usize;

    let claims = Claims {
        sub: user_id.to_string(),
        email: email.to_string(),
        exp: expiration,
    };

    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))
}

// ---------- Auth middleware (protecting routes) ----------

/// The verified identity of whoever made the request, extracted
/// from a valid JWT. Any handler that takes `AuthUser` as an
/// argument automatically requires a valid token — Axum runs
/// this extractor before the handler body executes, and refuses
/// the request with 401 if verification fails, so the handler
/// itself never has to think about "what if there's no token."
pub struct AuthUser {
    pub user_id: Uuid,
    pub email: String,
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get("Authorization")
            .and_then(|value| value.to_str().ok());

        let token = match auth_header {
            Some(header) if header.starts_with("Bearer ") => &header[7..],
            _ => {
                return Err(error_response(
                    StatusCode::UNAUTHORIZED,
                    "Missing or malformed Authorization header",
                ));
            }
        };

        let secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-only-insecure-secret".into());

        let decoded = decode::<Claims>(
            token,
            &DecodingKey::from_secret(secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| error_response(StatusCode::UNAUTHORIZED, "Invalid or expired token"))?;

        let user_id = Uuid::parse_str(&decoded.claims.sub)
            .map_err(|_| error_response(StatusCode::UNAUTHORIZED, "Invalid token subject"))?;

        Ok(AuthUser {
            user_id,
            email: decoded.claims.email,
        })
    }
}

// ---------- Handlers ----------

/// POST /auth/signup
pub async fn signup(
    State(db): State<Db>,
    ExtractJson(payload): ExtractJson<SignupRequest>,
) -> (StatusCode, Json<Value>) {
    let first_name = payload.first_name.trim().to_string();
    let last_name = payload.last_name.trim().to_string();
    let email = payload.email.trim().to_lowercase();

    if first_name.is_empty() || last_name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "First and last name are required");
    }
    if let Err(msg) = validate_email(&email) {
        return error_response(StatusCode::BAD_REQUEST, msg);
    }
    if let Err(msg) = validate_password(&payload.password) {
        return error_response(StatusCode::BAD_REQUEST, msg);
    }

    // Check for an existing account with this email.
    match crate::db::find_user_by_email(&db, &email).await {
        Ok(Some(_)) => {
            return error_response(StatusCode::CONFLICT, "An account with this email already exists");
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("DB error checking existing user: {e}");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong. Please try again.");
        }
    }

    // Hash the password — never store it plain, never store it
    // reversibly encrypted. bcrypt is one-way by design.
    let password_hash = match hash(&payload.password, DEFAULT_COST) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Hashing error: {e}");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong. Please try again.");
        }
    };

    let user_id = Uuid::new_v4();

    if let Err(e) = crate::db::insert_user(
        &db,
        user_id,
        &first_name,
        &last_name,
        &email,
        &password_hash,
    )
    .await
    {
        eprintln!("DB error inserting user: {e}");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Could not create account. Please try again.");
    }

    let token = match issue_token(&user_id.to_string(), &email) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("JWT error: {e}");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Account created, but sign-in failed. Please log in.");
        }
    };

    let user = UserPublic {
        id: user_id.to_string(),
        first_name,
        last_name,
        email,
    };

    (
        StatusCode::CREATED,
        Json(json!({ "status": "created", "user": user, "token": token })),
    )
}

/// GET /my-escrows
/// Requires a valid Bearer token. Returns every escrow where the
/// caller is either the client or the freelancer.
pub async fn my_escrows(
    State(db): State<Db>,
    auth_user: AuthUser,
) -> (StatusCode, Json<Value>) {
    match crate::db::find_escrows_for_user(&db, auth_user.user_id).await {
        Ok(rows) => {
            let escrows: Vec<crate::Escrow> = rows.into_iter().map(Into::into).collect();
            (StatusCode::OK, Json(json!({ "status": "ok", "escrows": escrows })))
        }
        Err(e) => {
            eprintln!("DB error fetching escrows: {e}");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "Could not load your escrows. Please try again.")
        }
    }
}

/// GET /users/lookup?email=someone@example.com
/// Requires a valid Bearer token (must be logged in to look
/// someone up). Returns only the minimum needed to confirm the
/// freelancer exists and show their name for confirmation —
/// never the password hash or any other account details.
#[derive(Debug, Deserialize)]
pub struct LookupQuery {
    pub email: String,
}

#[derive(Debug, Serialize)]
struct UserLookupResult {
    id: String,
    first_name: String,
    last_name: String,
}

pub async fn lookup_user(
    State(db): State<Db>,
    _auth_user: AuthUser,
    axum::extract::Query(query): axum::extract::Query<LookupQuery>,
) -> (StatusCode, Json<Value>) {
    let email = query.email.trim().to_lowercase();

    if let Err(msg) = validate_email(&email) {
        return error_response(StatusCode::BAD_REQUEST, msg);
    }

    match crate::db::find_user_by_email(&db, &email).await {
        Ok(Some(row)) => {
            let result = UserLookupResult {
                id: row.id.to_string(),
                first_name: row.first_name,
                last_name: row.last_name,
            };
            (StatusCode::OK, Json(json!({ "status": "ok", "user": result })))
        }
        Ok(None) => error_response(StatusCode::NOT_FOUND, "No Canopy account found with that email"),
        Err(e) => {
            eprintln!("DB error looking up user: {e}");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong. Please try again.")
        }
    }
}

/// POST /auth/login
pub async fn login(
    State(db): State<Db>,
    ExtractJson(payload): ExtractJson<LoginRequest>,
) -> (StatusCode, Json<Value>) {
    let email = payload.email.trim().to_lowercase();

    if let Err(msg) = validate_email(&email) {
        return error_response(StatusCode::BAD_REQUEST, msg);
    }
    if payload.password.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Enter your password");
    }

    let user_row = match crate::db::find_user_by_email(&db, &email).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            // Deliberately the same message as a wrong password —
            // never reveal whether an email is registered. This
            // prevents attackers from enumerating real accounts.
            return error_response(StatusCode::UNAUTHORIZED, "Invalid email or password");
        }
        Err(e) => {
            eprintln!("DB error finding user: {e}");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong. Please try again.");
        }
    };

    let password_ok = verify(&payload.password, &user_row.password_hash).unwrap_or(false);
    if !password_ok {
        return error_response(StatusCode::UNAUTHORIZED, "Invalid email or password");
    }

    let token = match issue_token(&user_row.id.to_string(), &user_row.email) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("JWT error: {e}");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong. Please try again.");
        }
    };

    let user = UserPublic {
        id: user_row.id.to_string(),
        first_name: user_row.first_name,
        last_name: user_row.last_name,
        email: user_row.email,
    };

    (
        StatusCode::OK,
        Json(json!({ "status": "ok", "user": user, "token": token })),
    )
}
