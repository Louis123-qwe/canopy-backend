// ============================================================
// CANOPY — Database helpers for auth
//
// Uses sqlx::query / query_as (runtime-checked), matching the
// pattern already used everywhere else in main.rs — not the
// sqlx::query_as! macro, which requires a live DB connection at
// compile time and isn't used elsewhere in this project.
// ============================================================

use sqlx::PgPool;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub password_hash: String,
}

/// Looks up a user by email. Returns None if no account exists —
/// this is not an error case, so it's Ok(None), not Err(...).
pub async fn find_user_by_email(
    db: &PgPool,
    email: &str,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        r#"
        SELECT id, first_name, last_name, email, password_hash
        FROM users
        WHERE email = $1
        "#,
    )
    .bind(email)
    .fetch_optional(db)
    .await
}

/// Inserts a new user row. The `users` table's UNIQUE constraint
/// on `email` is a second line of defense against the race where
/// two signups with the same email land at nearly the same time
/// — the handler already checks for an existing user first, but
/// the constraint catches it even if both checks pass together.
pub async fn insert_user(
    db: &PgPool,
    id: Uuid,
    first_name: &str,
    last_name: &str,
    email: &str,
    password_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO users (id, first_name, last_name, email, password_hash, created_at)
        VALUES ($1, $2, $3, $4, $5, now())
        "#,
    )
    .bind(id)
    .bind(first_name)
    .bind(last_name)
    .bind(email)
    .bind(password_hash)
    .execute(db)
    .await?;

    Ok(())
}

/// Returns every escrow row where the given user is either the
/// client or the freelancer. client_id/freelancer_id are stored
/// as TEXT on the escrows table (matching main.rs's existing
/// schema) but hold real user UUIDs as strings, so we compare
/// against the user's id cast to text.
pub async fn find_escrows_for_user(
    db: &PgPool,
    user_id: Uuid,
) -> Result<Vec<crate::EscrowRow>, sqlx::Error> {
    let user_id_str = user_id.to_string();

    sqlx::query_as::<_, crate::EscrowRow>(
        r#"
        SELECT id, client_id, freelancer_id, amount, description, status, milestones
        FROM escrows
        WHERE client_id = $1 OR freelancer_id = $1
        ORDER BY id DESC
        "#,
    )
    .bind(user_id_str)
    .fetch_all(db)
    .await
}
